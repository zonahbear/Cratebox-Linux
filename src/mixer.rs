//! The mixer (audio thread): per channel trim, 3-band isolator EQ (Linkwitz-Riley crossovers at
//! 250 Hz and 2.5 kHz, so each band can be killed), CFX filter, the Beat FX / Pad FX insert, headphone
//! cue send, meters, channel fader and crossfader; master level, limiter, cue/master headphone blend
//! and output routing. Four channels; 3 and 4 only sound in 4-deck mode.

use crate::fx::FxUnit;

pub const DECKS: usize = 4;

/// all controls normalised 0..1, like the hardware
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerParams {
    pub trim: [f32; DECKS],
    pub hi: [f32; DECKS],
    pub mid: [f32; DECKS],
    pub low: [f32; DECKS],
    pub cfx: [f32; DECKS],
    pub fader: [f32; DECKS],
    pub pfl: [bool; DECKS],
    pub xfader: f32,
    pub head_mix: f32,
    /// MST next to CUE/MASTER: the master in the headphones at full level
    pub master_cue: bool,
    pub master: f32,
    /// decks shown (3 and 4 only in 4-deck mode); hidden ones are muted
    pub active: [bool; DECKS],
}

impl Default for MixerParams {
    fn default() -> Self {
        MixerParams { trim: [0.5; DECKS], hi: [0.5; DECKS], mid: [0.5; DECKS], low: [0.5; DECKS], cfx: [0.5; DECKS], fader: [0.8; DECKS], pfl: [false; DECKS],
            xfader: 0.5, head_mix: 0.0, master_cue: false, master: 0.8, active: [true, true, false, false] }
    }
}

/// CUE/MASTER knob -> (cue gain, master gain): all cue up to 12 o'clock, then equal power to all master
pub fn head_mix(v: f64) -> (f64, f64) {
    let hm = ((v - 0.53) / 0.47).clamp(0.0, 1.0);
    ((hm * std::f64::consts::FRAC_PI_2).cos(), (hm * std::f64::consts::FRAC_PI_2).sin())
}

pub fn db_to_gain(db: f64) -> f64 { 10f64.powf(db / 20.0) }
/// left = kill, right = +6 dB
pub fn eq_gain(v: f64) -> f64 {
    if v < 0.5 { if v < 0.02 { 0.0 } else { (v / 0.5).powf(2.2) } } else { 1.0 + (v - 0.5) * 2.0 * (db_to_gain(6.0) - 1.0) }
}
pub fn trim_gain(v: f64) -> f64 { if v < 0.5 { if v < 0.01 { 0.0 } else { db_to_gain(-24.0 + 48.0 * v) } } else { db_to_gain((v - 0.5) * 18.0) } }
pub fn fader_gain(v: f64) -> f64 { if v < 0.005 { 0.0 } else { v.powf(1.8) } }
/// crossfader: both full at the centre, smooth fade to each side; decks 1 and 3 on the left, 2 and 4 on the right
pub fn xf_gain(i: usize, x: f64) -> f64 {
    let g = if i % 2 == 0 { if x <= 0.5 { 1.0 } else { ((x - 0.5) * std::f64::consts::PI).cos() } } else if x >= 0.5 { 1.0 } else { ((0.5 - x) * std::f64::consts::PI).cos() };
    if g < 0.002 { 0.0 } else { g }
}
/// CFX: the FLX4's per-channel filter knob. Left = low-pass, right = high-pass, centre = off.
/// Returns (lp freq, lp Q, hp freq, hp Q).
pub fn cfx_params(v: f64) -> (f64, f64, f64, f64) {
    if v < 0.47 { (60.0 * (20000.0f64 / 60.0).powf(v / 0.47), 1.4, 10.0, 0.707) }
    else if v > 0.53 { (22000.0, 0.707, 20.0 * (9000.0f64 / 20.0).powf((v - 0.53) / 0.47), 1.4) }
    else { (22000.0, 0.707, 10.0, 0.707) }
}

#[derive(Clone, Copy, Default)]
pub struct Biquad { b0: f64, b1: f64, b2: f64, a1: f64, a2: f64, z1: [f64; 2], z2: [f64; 2] }

impl Biquad {
    pub fn new(lp: bool, f: f64, q: f64, sr: f64) -> Biquad { let mut b = Biquad::default(); b.set(lp, f, q, sr); b }
    pub fn set(&mut self, lp: bool, f: f64, q: f64, sr: f64) {
        let f = f.min(sr * 0.49);
        let w = 2.0 * std::f64::consts::PI * f / sr;
        let (c, al) = (w.cos(), w.sin() / (2.0 * q));
        let a0 = 1.0 + al;
        let (b0, b1, b2) = if lp { ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0) } else { ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0) };
        self.b0 = b0 / a0; self.b1 = b1 / a0; self.b2 = b2 / a0; self.a1 = -2.0 * c / a0; self.a2 = (1.0 - al) / a0;
    }
    /// same coefficients, no ringing state
    pub fn cleared(&self) -> Biquad { Biquad { z1: [0.0; 2], z2: [0.0; 2], ..*self } }
    #[inline]
    pub fn tick(&mut self, ch: usize, x: f64) -> f64 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }
}

/// one-pole parameter smoothing, like Web Audio's setTargetAtTime(v, t, 8 ms)
#[derive(Clone, Copy)]
pub struct Smooth { pub v: f64, pub target: f64, k: f64 }
impl Smooth {
    pub fn new(v: f64, sr: f64) -> Smooth { Smooth { v, target: v, k: 1.0 - (-1.0 / (0.008 * sr)).exp() } }
    #[inline]
    pub fn next(&mut self) -> f64 { self.v += (self.target - self.v) * self.k; self.v }
    pub fn settled(&self) -> bool { (self.v - self.target).abs() < 1e-6 }
}

pub struct Strip {
    sr: f64,
    trim: Smooth,
    lo_g: Smooth,
    mid_g: Smooth,
    hi_g: Smooth,
    fader: Smooth,
    xf: Smooth,
    pfl: Smooth,
    lp_f: Smooth,
    lp_q: Smooth,
    hp_f: Smooth,
    hp_q: Smooth,
    lo: [Biquad; 2],
    mi: [Biquad; 4],
    hi: [Biquad; 2],
    lp: Biquad,
    hp: Biquad,
    /// Beat FX / Pad FX insert: after the EQ and CFX filter, before CUE, the meter and the fader
    pub fx: FxUnit,
    /// samples of silence in a row (an idle channel skips its filters)
    silent: usize,
    /// peak level of this channel (post-EQ/filter, pre-fader), for the channel meter
    pub peak: f32,
}

impl Strip {
    pub fn new(sr: f64) -> Strip {
        Strip {
            sr, trim: Smooth::new(1.0, sr), lo_g: Smooth::new(1.0, sr), mid_g: Smooth::new(1.0, sr), hi_g: Smooth::new(1.0, sr),
            fader: Smooth::new(fader_gain(0.8), sr), xf: Smooth::new(1.0, sr), pfl: Smooth::new(0.0, sr),
            lp_f: Smooth::new(22000.0, sr), lp_q: Smooth::new(0.707, sr), hp_f: Smooth::new(10.0, sr), hp_q: Smooth::new(0.707, sr),
            lo: [Biquad::new(true, 250.0, 0.7071, sr); 2],
            mi: [Biquad::new(false, 250.0, 0.7071, sr), Biquad::new(false, 250.0, 0.7071, sr), Biquad::new(true, 2500.0, 0.7071, sr), Biquad::new(true, 2500.0, 0.7071, sr)],
            hi: [Biquad::new(false, 2500.0, 0.7071, sr); 2],
            lp: Biquad::new(true, 22000.0, 0.707, sr), hp: Biquad::new(false, 10.0, 0.707, sr), fx: FxUnit::new(sr), silent: 0, peak: 0.0,
        }
    }
    pub fn set(&mut self, p: &MixerParams, i: usize) {
        self.trim.target = trim_gain(p.trim[i] as f64);
        self.hi_g.target = eq_gain(p.hi[i] as f64);
        self.mid_g.target = eq_gain(p.mid[i] as f64);
        self.lo_g.target = eq_gain(p.low[i] as f64);
        self.fader.target = fader_gain(p.fader[i] as f64);
        self.pfl.target = if p.pfl[i] { 1.0 } else { 0.0 };
        self.xf.target = if p.active[i] { xf_gain(i, p.xfader as f64) } else { 0.0 };
        let (lf, lq, hf, hq) = cfx_params(p.cfx[i] as f64);
        self.lp_f.target = lf; self.lp_q.target = lq; self.hp_f.target = hf; self.hp_q.target = hq;
    }
    /// process a block: in (l, r) -> adds to master (ml, mr) and cue bus (cl, cr). `l`/`r` are used as
    /// scratch (the FX insert works in place). `time` is the audio clock at the block's start.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], ml: &mut [f32], mr: &mut [f32], cl: &mut [f32], cr: &mut [f32], time: f64) {
        let n = l.len();
        // a stopped deck feeds silence: once the filters have rung out (half a second), skip the work.
        // The FX history keeps its timeline (rolls line up), and a ringing echo keeps the channel awake.
        if !self.fx.active() && l.iter().chain(r.iter()).all(|v| *v == 0.0) {
            self.silent += n;
            if self.silent as f64 > 0.5 * self.sr {
                self.lo = [self.lo[0].cleared(), self.lo[1].cleared()];
                for f in self.mi.iter_mut() { *f = f.cleared(); }
                self.hi = [self.hi[0].cleared(), self.hi[1].cleared()];
                self.lp = self.lp.cleared(); self.hp = self.hp.cleared();
                for s in [&mut self.trim, &mut self.lo_g, &mut self.mid_g, &mut self.hi_g, &mut self.fader, &mut self.xf, &mut self.pfl] { s.v = s.target; }
                self.fx.idle(n);
                self.peak = 0.0;
                return;
            }
        } else { self.silent = 0; }
        let mut i = 0;
        while i < n {
            // filter frequencies glide in 32-sample steps
            let e = (i + 32).min(n);
            if !(self.lp_f.settled() && self.lp_q.settled() && self.hp_f.settled() && self.hp_q.settled()) {
                for _ in i..e { self.lp_f.next(); self.lp_q.next(); self.hp_f.next(); self.hp_q.next(); }
                let sr = self.sr;
                self.lp.set(true, self.lp_f.v, self.lp_q.v, sr);
                self.hp.set(false, self.hp_f.v, self.hp_q.v, sr);
            }
            for k in i..e {
                let (tg, lg, mg, hg) = (self.trim.next(), self.lo_g.next(), self.mid_g.next(), self.hi_g.next());
                for ch in 0..2 {
                    let x = if ch == 0 { l[k] } else { r[k] } as f64 * tg;
                    let lo = self.lo[0].tick(ch, x);
                    let lo = self.lo[1].tick(ch, lo);
                    let mut mi = x;
                    for f in self.mi.iter_mut() { mi = f.tick(ch, mi); }
                    let hi = self.hi[0].tick(ch, x);
                    let hi = self.hi[1].tick(ch, hi);
                    let s = lo * lg + mi * mg + hi * hg;
                    let s = self.lp.tick(ch, s);
                    let y = self.hp.tick(ch, s) as f32;
                    if ch == 0 { l[k] = y } else { r[k] = y }
                }
            }
            i = e;
        }
        self.fx.process(l, r, time);
        let mut peak = 0f32;
        for k in 0..n {
            let (fg, xg, pg) = (self.fader.next(), self.xf.next(), self.pfl.next());
            let (a, b) = (l[k] as f64, r[k] as f64);
            let pk = l[k].abs().max(r[k].abs());
            if pk > peak { peak = pk; }
            let g = fg * xg;
            ml[k] += (a * g) as f32; mr[k] += (b * g) as f32;
            cl[k] += (a * pg) as f32; cr[k] += (b * pg) as f32;
        }
        self.peak = peak;
    }
}

/// peak limiter on the master: threshold -1 dBFS, ratio 20, 1 ms attack, 80 ms release
pub struct Limiter { env: f64, att: f64, rel: f64, thr_db: f64 }
impl Limiter {
    pub fn new(sr: f64) -> Limiter { Limiter { env: -120.0, att: 1.0 - (-1.0 / (0.001 * sr)).exp(), rel: 1.0 - (-1.0 / (0.08 * sr)).exp(), thr_db: -1.0 } }
    #[inline]
    pub fn gain(&mut self, peak: f64) -> f64 {
        let db = 20.0 * (peak + 1e-12).log10();
        let k = if db > self.env { self.att } else { self.rel };
        self.env += (db - self.env) * k;
        let over = self.env - self.thr_db;
        if over > 0.0 { db_to_gain(-over * (1.0 - 1.0 / 20.0)) } else { 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// stereo master only
    Master,
    /// master on outputs 1-2, headphones on 3-4 (the FLX4's own sound card)
    Quad,
    /// mono master left, mono cue right, for a splitter cable
    Split,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curves() {
        assert_eq!(eq_gain(0.0), 0.0);
        assert!((eq_gain(0.5) - 1.0).abs() < 1e-12);
        assert!((eq_gain(1.0) - db_to_gain(6.0)).abs() < 1e-9);
        assert!((xf_gain(0, 0.5) - 1.0).abs() < 1e-12 && xf_gain(1, 0.0) == 0.0 && xf_gain(0, 1.0) == 0.0);
        assert_eq!(fader_gain(0.0), 0.0);
        assert!(xf_gain(2, 1.0) == 0.0 && xf_gain(3, 0.0) == 0.0, "3 left, 4 right");
        assert_eq!(head_mix(0.0), (1.0, 0.0));
        assert!(head_mix(1.0).0.abs() < 1e-12 && (head_mix(1.0).1 - 1.0).abs() < 1e-12);
    }
    #[test]
    fn eq_bands_sum_flat_and_kill_works() {
        let sr = 48000.0;
        let tone = |f: f64| -> Vec<f32> { (0..48000).map(|i| (0.5 * (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin()) as f32).collect() };
        let level = |p: &MixerParams, f: f64| -> f32 {
            let mut s = Strip::new(sr);
            s.set(p, 0);
            for _ in 0..3 { s.fader.next(); }
            let x = tone(f);
            let n = x.len();
            let (mut xl, mut xr) = (x.clone(), x.clone());
            let (mut ml, mut mr, mut cl, mut cr) = (vec![0f32; n], vec![0f32; n], vec![0f32; n], vec![0f32; n]);
            s.process(&mut xl, &mut xr, &mut ml, &mut mr, &mut cl, &mut cr, 0.0);
            ml[24000..].iter().fold(0f32, |m, v| m.max(v.abs()))
        };
        let mut p = MixerParams { fader: [1.0; DECKS], ..Default::default() };
        for f in [60.0, 800.0, 6000.0] { let l = level(&p, f); assert!((l - 0.5).abs() < 0.06, "flat at {f}: {l}"); }
        p.low = [0.0; DECKS];
        assert!(level(&p, 60.0) < 0.02, "low kill");
        assert!(level(&p, 6000.0) > 0.45, "highs untouched by low kill");
        p.low = [0.5; DECKS];
        p.cfx = [0.1; DECKS];
        assert!(level(&p, 6000.0) < 0.05, "CFX left = low-pass");
    }
}
