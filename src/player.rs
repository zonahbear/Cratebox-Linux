//! Deck player (runs on the audio thread). With master tempo (keylock) on, the playhead still moves at
//! the tempo rate, but the sound is rebuilt from 46 ms grains played at normal speed (WSOLA): each new
//! grain is taken near the playhead, at the offset that best lines up with the previous grain's tail.
//! A port of the web version's AudioWorklet; it works in 128-sample blocks like Web Audio does.

use std::sync::Arc;

pub type Pcm = Arc<Vec<f32>>;

#[derive(Clone)]
pub enum PlayerCmd {
    Unload,
    Load { l: Pcm, r: Pcm, mt: bool, spin: f64 },
    Play(bool),
    Rate(f64),
    Seek { pos: f64, seq: u64 },
    Bend(f64),
    Scratch { on: bool, no_coast: bool },
    Spin(f64),
    Jog(f64),
    Loop { on: bool, a: f64, b: f64 },
    Mt(bool),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub pos: f64,
    pub playing: bool,
    pub ended: bool,
    pub speed: f64,
    pub coast: bool,
    pub seq: u64,
    pub loaded: bool,
}

pub const BLOCK: usize = 128;

pub struct Player {
    sr: f64,
    l: Option<Pcm>,
    r: Option<Pcm>,
    len: usize,
    pub pos: f64,
    pub playing: bool,
    rate: f64,
    bend: f64,
    scratching: bool,
    target: f64,
    vel: f64,
    loop_on: bool,
    loop_in: f64,
    loop_out: f64,
    amp: f64,
    pub ended: bool,
    coast: bool,
    spin_a: f64,
    seq: u64,
    // keylock
    mt: bool,
    h: usize,
    nw: usize,
    s: usize,
    ramp: usize,
    win: Vec<f32>,
    g_s: [f64; 2],
    g_k: [usize; 2],
    g_f: [bool; 2],
    cur: usize,
    ph: usize,
    prev_s: f64,
    fresh: bool,
    buf: Vec<f32>,
    tpl: Vec<f32>,
    /// buffers replaced on this thread, handed back so they're freed elsewhere
    pub trash: Vec<Pcm>,
}

impl Player {
    pub fn new(sr: f64) -> Player {
        let h = (0.023 * sr).round() as usize;
        let nw = 2 * h;
        let s = (0.013 * sr).round() as usize;
        let win = (0..nw).map(|k| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / nw as f64).cos()) as f32).collect();
        let mut p = Player {
            sr, l: None, r: None, len: 0, pos: 0.0, playing: false, rate: 1.0, bend: 0.0, scratching: false, target: 0.0, vel: 0.0,
            loop_on: false, loop_in: 0.0, loop_out: 0.0, amp: 0.0, ended: false, coast: false, spin_a: 9.0 / 2.5, seq: 0,
            mt: false, h, nw, s, ramp: (0.002 * sr).round() as usize, win, g_s: [0.0; 2], g_k: [nw; 2], g_f: [false; 2], cur: 0, ph: 0,
            prev_s: 0.0, fresh: true, buf: vec![0.0; 2 * s + h + 8], tpl: vec![0.0; h], trash: vec![],
        };
        p.flush();
        p
    }
    fn flush(&mut self) { self.g_k = [self.nw; 2]; self.ph = 0; self.fresh = true; }

    pub fn cmd(&mut self, c: PlayerCmd) {
        match c {
            PlayerCmd::Unload => {
                if let Some(x) = self.l.take() { self.trash.push(x); }
                if let Some(x) = self.r.take() { self.trash.push(x); }
                self.len = 0; self.playing = false; self.amp = 0.0;
            }
            PlayerCmd::Load { l, r, mt, spin } => {
                if let Some(x) = self.l.take() { self.trash.push(x); }
                if let Some(x) = self.r.take() { self.trash.push(x); }
                self.len = l.len().min(r.len());
                self.l = Some(l); self.r = Some(r);
                self.pos = 0.0; self.target = 0.0; self.playing = false; self.loop_on = false; self.amp = 0.0; self.vel = 0.0; self.ended = false;
                self.mt = mt;
                if spin > 0.0 { self.spin_a = 9.0 / spin; }
                self.coast = false; self.flush();
            }
            PlayerCmd::Play(v) => { self.playing = v; if v { self.ended = false; } }
            PlayerCmd::Rate(v) => self.rate = v,
            PlayerCmd::Seek { pos, seq } => {
                self.seq = seq;
                self.pos = pos.min(self.len as f64 - 2.0).max(0.0);
                self.target = self.pos; self.vel = 0.0; self.coast = false; self.flush();
            }
            PlayerCmd::Bend(v) => self.bend = (self.bend + v).clamp(-0.6, 0.6),
            PlayerCmd::Scratch { on, no_coast } => {
                if on {
                    self.vel = if self.coast { self.vel } else if self.playing { self.rate } else { 0.0 };
                    self.coast = false;
                } else {
                    // let go mid-fling (backwards, or well off the motor speed): keep spinning and coast back
                    let goal = if self.playing { self.rate } else { 0.0 };
                    self.vel = self.vel.clamp(-10.0, 10.0);
                    self.coast = !no_coast && (self.vel < -0.5 || (self.vel - goal).abs() > 1.5);
                    if !self.coast { self.vel = 0.0; }
                }
                self.scratching = on; self.target = self.pos; self.flush();
            }
            PlayerCmd::Spin(v) => { if v > 0.0 { self.spin_a = 9.0 / v; } }
            PlayerCmd::Jog(v) => self.target += v,
            PlayerCmd::Loop { on, a, b } => { self.loop_on = on; self.loop_in = a; self.loop_out = b; }
            PlayerCmd::Mt(v) => { self.mt = v; self.flush(); }
        }
    }

    pub fn report(&self) -> Report {
        let speed = if self.scratching || self.coast { self.vel } else if self.playing { self.rate * (1.0 + self.bend) } else { 0.0 };
        Report { pos: self.pos, playing: self.playing, ended: self.ended, speed, coast: self.coast, seq: self.seq, loaded: self.l.is_some() }
    }

    /// source index for keylock reads: wraps inside an active loop; None = silence
    #[inline]
    fn idx(&self, p: f64) -> Option<usize> {
        let mut p = p;
        if self.loop_on && self.loop_out > self.loop_in && p >= self.loop_out {
            let w = self.loop_out - self.loop_in;
            p = self.loop_in + ((p - self.loop_in) % w);
        }
        let p = p.trunc();
        if p >= 0.0 && (p as usize) < self.len { Some(p as usize) } else { None }
    }
    #[inline]
    fn mono(&self, l: &[f32], r: &[f32], p: f64) -> f32 { match self.idx(p) { Some(j) => l[j] + r[j], None => 0.0 } }

    fn start_grain(&mut self, l: &[f32], r: &[f32], pos: f64, v: f64) {
        let (h, s) = (self.h, self.s);
        self.cur ^= 1;
        let slot = self.cur;
        if self.fresh {
            let st = pos.round();
            self.g_s[slot] = st; self.g_k[slot] = 0; self.g_f[slot] = true; self.g_k[slot ^ 1] = self.nw;
            self.prev_s = st; self.fresh = false;
            return;
        }
        let s0 = (pos + (v - 1.0) * h as f64).round(); // keeps each grain centred on the playhead
        let nat = self.prev_s + h as f64; // where the previous grain would continue
        let mut best = 0.0;
        if nat >= s0 - s as f64 && nat <= s0 + s as f64 && (v - 1.0).abs() < 1e-6 {
            best = nat - s0;
        } else {
            let base = s0 - s as f64;
            let mut te = 0f32;
            for k in 0..h { let x = self.mono(l, r, nat + k as f64); self.tpl[k] = x; te += x * x; }
            if te > 1e-9 {
                let bn = 2 * s + h;
                for k in 0..bn { self.buf[k] = self.mono(l, r, base + k as f64); }
                let tpl = &self.tpl;
                let buf = &self.buf;
                let score = |o: usize, step: usize| -> f32 {
                    let (mut c, mut e) = (0f32, 1e-9f32);
                    let mut k = 0;
                    while k < h { let y = buf[o + k]; c += tpl[k] * y; e += y * y; k += step; }
                    c / e.sqrt()
                };
                let mut bs = f32::NEG_INFINITY;
                let mut bo = s;
                let mut o = 0;
                while o <= 2 * s { let sc = score(o, 8); if sc > bs { bs = sc; bo = o; } o += 4; }
                let lo = bo.saturating_sub(4);
                let hi = (2 * s).min(bo + 4);
                bs = f32::NEG_INFINITY;
                let mut fo = bo;
                for o in lo..=hi { let sc = score(o, 1); if sc > bs { bs = sc; fo = o; } }
                best = fo as f64 - s as f64;
            }
        }
        let st = s0 + best;
        self.g_s[slot] = st; self.g_k[slot] = 0; self.g_f[slot] = false; self.prev_s = st;
    }

    /// render into out_l/out_r (overwrites); any length, worked in 128-sample blocks
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        let mut i = 0;
        while i < n {
            let e = (i + BLOCK).min(n);
            self.block(&mut out_l[i..e], &mut out_r[i..e]);
            i = e;
        }
    }

    fn block(&mut self, o_l: &mut [f32], o_r: &mut [f32]) {
        o_l.fill(0.0); o_r.fill(0.0);
        let (lp, rp) = match (&self.l, &self.r) { (Some(l), Some(r)) => (l.clone(), r.clone()), _ => return };
        let (l, r) = (&lp[..], &rp[..]);
        let len = self.len as f64;
        let sr = self.sr;
        let mut pos = self.pos;
        let mut vel = self.vel;
        let mut amp = self.amp;
        let follow = 1.0 / (0.012 * sr); // scratch: catch up with the hand in about 12 ms
        let amp_step = 1.0 / (0.005 * sr); // 5 ms fades on start/stop: no clicks
        let use_mt = self.mt && !self.scratching && !self.coast;
        let spin_step = self.spin_a / sr;
        let (nw, h, ramp) = (self.nw, self.h, self.ramp);
        for i in 0..o_l.len() {
            let (v, want);
            if self.scratching {
                vel += ((self.target - pos) * follow - vel) * 0.02;
                v = vel; want = 1.0;
            } else if self.coast {
                let goal = if self.playing { self.rate } else { 0.0 };
                if vel < goal { vel = (vel + spin_step).min(goal); } else { vel = (vel - spin_step).max(goal); }
                if vel == goal { self.coast = false; }
                v = vel; want = 1.0;
            } else {
                want = if self.playing { 1.0 } else { 0.0 };
                v = if self.playing { self.rate * (1.0 + self.bend) } else if amp > 0.0 { self.rate } else { 0.0 };
            }
            if want > amp { amp = (amp + amp_step).min(want); } else if want < amp { amp = (amp - amp_step).max(want); }
            if amp > 0.0001 && pos >= 0.0 && pos < len - 1.0 {
                if use_mt {
                    if self.ph == 0 { self.start_grain(l, r, pos, v); }
                    let (mut sl, mut sr_) = (0f32, 0f32);
                    for g in 0..2 {
                        let k = self.g_k[g];
                        if k >= nw { continue; }
                        if let Some(j) = self.idx(self.g_s[g] + k as f64) {
                            let w = if self.g_f[g] && k < h { if k < ramp { k as f32 / ramp as f32 } else { 1.0 } } else { self.win[k] };
                            sl += l[j] * w; sr_ += r[j] * w;
                        }
                        self.g_k[g] = k + 1;
                    }
                    o_l[i] = sl * amp as f32; o_r[i] = sr_ * amp as f32;
                    self.ph += 1;
                    if self.ph >= h { self.ph = 0; }
                } else {
                    let i0 = pos as usize;
                    let f = (pos - i0 as f64) as f32;
                    o_l[i] = (l[i0] + (l[i0 + 1] - l[i0]) * f) * amp as f32;
                    o_r[i] = (r[i0] + (r[i0 + 1] - r[i0]) * f) * amp as f32;
                }
            } else if use_mt && !self.fresh {
                self.flush();
            }
            pos += v;
            if self.loop_on && v > 0.0 && pos >= self.loop_out && self.loop_out > self.loop_in {
                let w = self.loop_out - self.loop_in;
                pos -= w;
                if self.scratching { self.target -= w; }
            }
            if pos < 0.0 { pos = 0.0; if self.target < 0.0 { self.target = 0.0; } }
            if pos >= len - 1.0 { pos = len - 1.0; if self.playing { self.playing = false; self.ended = true; } }
        }
        self.pos = pos; self.vel = vel; self.amp = amp;
        self.bend *= 0.96;
        if self.bend.abs() < 1e-5 { self.bend = 0.0; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(sr: f64, secs: f64, f: f64) -> Pcm { Arc::new((0..(sr * secs) as usize).map(|i| (0.5 * (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin()) as f32).collect()) }
    /// frequency from zero crossings over a stretch of output
    fn freq(x: &[f32], sr: f64) -> f64 {
        let mut c = vec![];
        for i in 1..x.len() { if x[i - 1] < 0.0 && x[i] >= 0.0 { c.push(i as f64 - x[i] as f64 / (x[i] - x[i - 1]) as f64); } }
        (c.len() - 1) as f64 / ((c[c.len() - 1] - c[0]) / sr)
    }
    fn run(p: &mut Player, secs: f64, sr: f64) -> Vec<f32> {
        let n = (secs * sr) as usize;
        let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
        p.process(&mut l, &mut r);
        l
    }

    #[test]
    fn plays_at_rate_and_vinyl_shifts_pitch() {
        let sr = 48000.0;
        let t = tone(sr, 4.0, 440.0);
        let mut p = Player::new(sr);
        p.cmd(PlayerCmd::Load { l: t.clone(), r: t, mt: false, spin: 2.5 });
        p.cmd(PlayerCmd::Rate(1.08));
        p.cmd(PlayerCmd::Play(true));
        let out = run(&mut p, 1.0, sr);
        assert!((p.pos / sr - 1.08).abs() < 0.01, "moved {}", p.pos / sr);
        let f = freq(&out[4800..], sr);
        assert!((f - 475.2).abs() < 1.5, "vinyl {f}");
    }

    #[test]
    fn keylock_keeps_pitch_and_level() {
        let sr = 48000.0;
        let t = tone(sr, 6.0, 440.0);
        let mut p = Player::new(sr);
        p.cmd(PlayerCmd::Load { l: t.clone(), r: t, mt: true, spin: 2.5 });
        p.cmd(PlayerCmd::Rate(1.08));
        p.cmd(PlayerCmd::Play(true));
        let out = run(&mut p, 2.0, sr);
        assert!((p.pos / sr - 2.16).abs() < 0.02, "moved {}", p.pos / sr);
        let f = freq(&out[9600..], sr);
        assert!((f - 440.0).abs() < 2.0, "keylock {f}");
        // envelope stays steady (no warble): min/max of 10 ms peak windows
        let w = 480;
        let pk: Vec<f32> = out[9600..].chunks(w).map(|c| c.iter().fold(0f32, |m, v| m.max(v.abs()))).collect();
        let (mn, mx) = pk.iter().fold((f32::MAX, 0f32), |a, &v| (a.0.min(v), a.1.max(v)));
        assert!(mn / mx > 0.85, "warble {}", mn / mx);
    }

    #[test]
    fn loops_wrap_and_seek_seq_reports() {
        let sr = 44100.0;
        let t = tone(sr, 10.0, 220.0);
        let mut p = Player::new(sr);
        p.cmd(PlayerCmd::Load { l: t.clone(), r: t, mt: false, spin: 2.5 });
        p.cmd(PlayerCmd::Seek { pos: sr, seq: 7 });
        p.cmd(PlayerCmd::Loop { on: true, a: sr, b: 1.5 * sr });
        p.cmd(PlayerCmd::Play(true));
        run(&mut p, 2.0, sr);
        let rep = p.report();
        assert!(rep.pos >= sr && rep.pos < 1.5 * sr, "pos {}", rep.pos / sr);
        assert_eq!(rep.seq, 7);
    }

    #[test]
    fn backspin_coasts_back_to_speed() {
        let sr = 44100.0;
        let t = tone(sr, 20.0, 220.0);
        let mut p = Player::new(sr);
        p.cmd(PlayerCmd::Load { l: t.clone(), r: t, mt: true, spin: 1.0 });
        p.cmd(PlayerCmd::Seek { pos: 10.0 * sr, seq: 1 });
        p.cmd(PlayerCmd::Play(true));
        run(&mut p, 0.2, sr);
        p.cmd(PlayerCmd::Scratch { on: true, no_coast: false });
        // fling it backwards fast
        for _ in 0..20 { p.cmd(PlayerCmd::Jog(-0.3 * sr)); run(&mut p, 0.01, sr); }
        let before = p.pos;
        p.cmd(PlayerCmd::Scratch { on: false, no_coast: false });
        assert!(p.report().coast, "should coast after a backwards fling");
        run(&mut p, 0.3, sr);
        assert!(p.pos < before, "still spinning backwards");
        run(&mut p, 3.0, sr);
        let r = p.report();
        assert!(!r.coast && (r.speed - 1.0).abs() < 1e-9, "back to speed {}", r.speed);
    }

    #[test]
    fn stops_at_the_end() {
        let sr = 44100.0;
        let t = tone(sr, 1.0, 220.0);
        let mut p = Player::new(sr);
        p.cmd(PlayerCmd::Load { l: t.clone(), r: t, mt: false, spin: 2.5 });
        p.cmd(PlayerCmd::Play(true));
        run(&mut p, 1.5, sr);
        let r = p.report();
        assert!(!r.playing && r.ended);
    }
}
