//! The audio engine: four deck players into the mixer (3 and 4 sound only in 4-deck mode), Beat FX on
//! the master, rendered into whatever the output device wants.
//! Commands come in over a channel; the state the UI needs goes out through a shared snapshot that
//! the audio thread only ever try-locks (it never waits).

use crate::fx::{FxClock, FxUnit, SlotSet};
use crate::mixer::{head_mix, Limiter, MixerParams, Route, Smooth, Strip, DECKS};
use crate::player::{Pcm, Player, PlayerCmd, Report};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub enum Cmd {
    Deck(usize, PlayerCmd),
    Mixer(MixerParams),
    Route(Route),
    /// three short beeps in the headphone mix only
    TestCue,
    /// set an FX slot on a unit (0-3 = the deck channels, 4 = master); `pad` picks the PAD FX slot
    Fx { unit: usize, pad: bool, set: SlotSet },
    Clock { unit: usize, clock: FxClock },
    /// PUFF (HEAT): how hard the Puffco is being hit, 0..1, for that unit's BEAT FX slot
    FxMod { unit: usize, v: f64 },
    /// the master (post-limiter) goes here as interleaved stereo while recording
    Record(Option<crate::ring::Producer>),
}

/// unit number of the master insert
pub const MASTER_FX: usize = DECKS;

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub decks: [Report; DECKS],
    /// when this snapshot was taken (end of the block just rendered)
    pub at: Instant,
    /// the audio clock (seconds of output rendered) at `at`
    pub time: f64,
    pub meters: [f32; DECKS],
    pub master_meter: [f32; 2],
    pub sample_rate: f64,
    pub channels: usize,
    pub blocks: u64,
}

impl Default for Status {
    fn default() -> Self { Status { decks: [Report::default(); DECKS], at: Instant::now(), time: 0.0, meters: [0.0; DECKS], master_meter: [0.0; 2], sample_rate: 48000.0, channels: 2, blocks: 0 } }
}

pub type Shared = Arc<Mutex<Status>>;

pub struct Engine {
    pub sr: f64,
    pub players: [Player; DECKS],
    strips: [Strip; DECKS],
    master_fx: FxUnit,
    params: MixerParams,
    master_level: Smooth,
    hp_cue: Smooth,
    hp_master: Smooth,
    master_cue: Smooth,
    frames: u64,
    rec: Option<crate::ring::Producer>,
    limiter: Limiter,
    pub route: Route,
    rx: Receiver<Cmd>,
    trash: Sender<Pcm>,
    pub shared: Shared,
    beep_left: usize,
    beep_t: usize,
    // scratch buffers
    buf: Vec<Vec<f32>>,
    meter_hold: [f32; DECKS],
    mm_hold: [f32; 2],
    blocks: u64,
    /// headphone feed for a second output device (cue mix, interleaved stereo)
    pub hp_tap: Option<crate::ring::Producer>,
}

pub fn master_level_gain(v: f64) -> f64 { v.powf(1.6) * 1.2 }

impl Engine {
    pub fn new(sr: f64, rx: Receiver<Cmd>, trash: Sender<Pcm>, shared: Shared) -> Engine {
        let p = MixerParams::default();
        let mut e = Engine {
            sr, players: std::array::from_fn(|_| Player::new(sr)), strips: std::array::from_fn(|_| Strip::new(sr)), master_fx: FxUnit::new(sr), params: p,
            master_level: Smooth::new(master_level_gain(p.master as f64), sr), hp_cue: Smooth::new(0.0, sr), hp_master: Smooth::new(0.0, sr), master_cue: Smooth::new(0.0, sr),
            frames: 0, rec: None, limiter: Limiter::new(sr), route: Route::Master, rx, trash, shared, beep_left: 0, beep_t: 0,
            buf: vec![vec![0.0; 4096]; 6], meter_hold: [0.0; DECKS], mm_hold: [0.0; 2], blocks: 0, hp_tap: None,
        };
        e.apply(p);
        e
    }
    fn apply(&mut self, p: MixerParams) {
        self.params = p;
        for (i, s) in self.strips.iter_mut().enumerate() { s.set(&p, i); }
        self.master_level.target = master_level_gain(p.master as f64);
        let (c, m) = head_mix(p.head_mix as f64);
        self.hp_cue.target = c;
        self.hp_master.target = m;
        self.master_cue.target = if p.master_cue { 1.0 } else { 0.0 };
    }
    fn drain(&mut self) {
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Deck(i, pc) => self.players[i].cmd(pc),
                Cmd::Mixer(p) => self.apply(p),
                Cmd::Route(r) => self.route = r,
                Cmd::TestCue => { self.beep_left = (0.9 * self.sr) as usize; self.beep_t = 0; }
                Cmd::Fx { unit, pad, set } => { let u = self.fx_unit(unit); if pad { u.pad.set(set) } else { u.beat.set(set) } }
                Cmd::Clock { unit, clock } => self.fx_unit(unit).clock = clock,
                Cmd::FxMod { unit, v } => self.fx_unit(unit).beat.set_mod(v),
                // (the recorder thread holds the other end for a while after it stops, so this never frees)
                Cmd::Record(r) => self.rec = r,
            }
        }
        for p in self.players.iter_mut() {
            for x in p.trash.drain(..) { let _ = self.trash.send(x); }
        }
    }

    fn fx_unit(&mut self, u: usize) -> &mut FxUnit { if u < DECKS { &mut self.strips[u].fx } else { &mut self.master_fx } }

    /// fill an interleaved output buffer of `channels` channels
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        let frames = out.len() / channels.max(1);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(1024);
            self.render_block(&mut out[done * channels..(done + n) * channels], channels, n);
            done += n;
        }
    }

    fn render_block(&mut self, out: &mut [f32], channels: usize, n: usize) {
        self.drain();
        let time = self.frames as f64 / self.sr;
        let [dl, dr, ml, mr, cl, cr] = &mut self.buf[..] else { unreachable!() };
        for b in [&mut *ml, &mut *mr, &mut *cl, &mut *cr] { b[..n].fill(0.0); }
        for i in 0..DECKS {
            // a hidden deck (3/4 outside 4-deck mode) is silent; it's stopped when hidden, so this costs nothing
            self.players[i].process(&mut dl[..n], &mut dr[..n]);
            self.strips[i].process(&mut dl[..n], &mut dr[..n], &mut ml[..n], &mut mr[..n], &mut cl[..n], &mut cr[..n], time);
        }
        if self.master_fx.active() { self.master_fx.process(&mut ml[..n], &mut mr[..n], time); } else { self.master_fx.idle(n); }
        let mut mpk = [0f32; 2];
        for k in 0..n {
            let g = self.master_level.next();
            let (hc, hm) = (self.hp_cue.next(), self.hp_master.next() + self.master_cue.next());
            let (l, r) = (ml[k] as f64 * g, mr[k] as f64 * g);
            // headphones: blend of the cue bus and the master (before the limiter, like the web version)
            let mut beep = 0.0;
            if self.beep_left > 0 {
                let t = self.beep_t as f64 / self.sr;
                let ph = t % 0.3;
                if ph < 0.17 { let env = (ph / 0.01).min(1.0).min(((0.17 - ph) / 0.02).min(1.0)); beep = 0.25 * env * (2.0 * std::f64::consts::PI * 880.0 * t).sin(); }
                self.beep_t += 1; self.beep_left -= 1;
            }
            let hl = cl[k] as f64 * hc + l * hm + beep;
            let hr = cr[k] as f64 * hc + r * hm + beep;
            let lg = self.limiter.gain(l.abs().max(r.abs()));
            let (l, r) = ((l * lg) as f32, (r * lg) as f32);
            mpk[0] = mpk[0].max(l.abs()); mpk[1] = mpk[1].max(r.abs());
            ml[k] = l; mr[k] = r; cl[k] = hl as f32; cr[k] = hr as f32;
        }
        let o = out;
        for x in o.iter_mut() { *x = 0.0; }
        for k in 0..n {
            let f = &mut o[k * channels..(k + 1) * channels];
            match self.route {
                Route::Quad if channels >= 4 => { f[0] = ml[k]; f[1] = mr[k]; f[2] = cl[k]; f[3] = cr[k]; }
                Route::Split if channels >= 2 => { f[0] = 0.5 * (ml[k] + mr[k]); f[1] = 0.5 * (cl[k] + cr[k]); }
                _ => { if channels >= 2 { f[0] = ml[k]; f[1] = mr[k]; } else { f[0] = 0.5 * (ml[k] + mr[k]); } }
            }
        }
        if let Some(tap) = self.hp_tap.as_mut() {
            for k in 0..n { tap.push2(cl[k], cr[k]); }
        }
        if let Some(rec) = self.rec.as_mut() {
            for k in 0..n { rec.push2(ml[k], mr[k]); }
        }
        self.blocks += 1;
        self.frames += n as u64;
        // meters: peak with a short hold/decay
        for i in 0..DECKS {
            let p = self.strips[i].peak;
            self.meter_hold[i] = if p > self.meter_hold[i] { p } else { self.meter_hold[i] * 0.85 };
        }
        for i in 0..2 { self.mm_hold[i] = if mpk[i] > self.mm_hold[i] { mpk[i] } else { self.mm_hold[i] * 0.85 }; }
        if let Ok(mut s) = self.shared.try_lock() {
            s.decks = std::array::from_fn(|i| self.players[i].report());
            s.at = Instant::now();
            s.time = self.frames as f64 / self.sr;
            s.meters = self.meter_hold;
            s.master_meter = self.mm_hold;
            s.sample_rate = self.sr;
            s.channels = channels;
            s.blocks = self.blocks;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    fn tone(sr: f64, secs: f64, f: f64) -> Pcm { Arc::new((0..(sr * secs) as usize).map(|i| (0.5 * (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin()) as f32).collect()) }
    fn dom_freq(x: &[f32], sr: f64) -> f64 {
        let mut c = vec![];
        for i in 1..x.len() { if x[i - 1] < 0.0 && x[i] >= 0.0 { c.push(i as f64); } }
        if c.len() < 2 { return 0.0; }
        (c.len() - 1) as f64 / ((c[c.len() - 1] - c[0]) / sr)
    }
    #[test]
    fn quad_routing_keeps_cue_out_of_the_master() {
        let sr = 48000.0;
        let (tx, rx) = channel();
        let (ttx, _trx) = channel();
        let mut e = Engine::new(sr, rx, ttx, Arc::new(Mutex::new(Status::default())));
        let a = tone(sr, 3.0, 220.0);
        let b = tone(sr, 3.0, 880.0);
        tx.send(Cmd::Deck(0, PlayerCmd::Load { l: a.clone(), r: a, mt: false, spin: 2.5 })).unwrap();
        tx.send(Cmd::Deck(1, PlayerCmd::Load { l: b.clone(), r: b, mt: false, spin: 2.5 })).unwrap();
        let p = MixerParams { fader: [1.0, 0.0, 0.0, 0.0], pfl: [false, true, false, false], head_mix: 0.0, ..Default::default() };
        tx.send(Cmd::Mixer(p)).unwrap();
        tx.send(Cmd::Route(Route::Quad)).unwrap();
        tx.send(Cmd::Deck(0, PlayerCmd::Play(true))).unwrap();
        tx.send(Cmd::Deck(1, PlayerCmd::Play(true))).unwrap();
        let mut out = vec![0f32; 48000 * 4];
        e.render(&mut out, 4);
        let ch = |c: usize| -> Vec<f32> { out.chunks(4).skip(24000).map(|f| f[c]).collect() };
        let (m, h) = (ch(0), ch(2));
        assert!((dom_freq(&m, sr) - 220.0).abs() < 3.0, "master {}", dom_freq(&m, sr));
        assert!((dom_freq(&h, sr) - 880.0).abs() < 5.0, "cue {}", dom_freq(&h, sr));
        let s = e.shared.lock().unwrap();
        assert!(s.decks[0].playing && s.meters[0] > 0.3 && s.master_meter[0] > 0.1);
    }
    #[test]
    fn limiter_holds_the_peak() {
        let sr = 48000.0;
        let (tx, rx) = channel();
        let (ttx, _trx) = channel();
        let mut e = Engine::new(sr, rx, ttx, Arc::new(Mutex::new(Status::default())));
        let a: Pcm = Arc::new(vec![0.99f32; 96000].iter().enumerate().map(|(i, v)| if i % 2 == 0 { *v } else { -*v }).collect());
        tx.send(Cmd::Deck(0, PlayerCmd::Load { l: a.clone(), r: a, mt: false, spin: 2.5 })).unwrap();
        tx.send(Cmd::Mixer(MixerParams { fader: [1.0; DECKS], master: 1.0, trim: [1.0; DECKS], ..Default::default() })).unwrap();
        tx.send(Cmd::Deck(0, PlayerCmd::Play(true))).unwrap();
        let mut out = vec![0f32; 48000 * 2];
        e.render(&mut out, 2);
        let pk = out[48000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(pk < 0.95, "peak {pk}");
    }
}
