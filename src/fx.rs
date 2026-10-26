//! Beat FX / Pad FX (audio thread): a line-by-line port of the web app's FxUnit worklet, by way of the
//! iOS port (CrateboxKit FxUnit.swift). One unit per insert point (every deck channel, and the master),
//! two slots: "beat" (the BEAT FX unit, when it's routed here) and "pad" (PAD FX, decks only). Timed by a
//! beat clock (where the beat is now and how fast it's moving), so echoes, rolls and gates land on the
//! grid. With nothing on and no tails left it's a straight copy. Every buffer is allocated up front: the
//! audio thread never allocates here.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FxKind { Echo, Delay, PingPong, Reverb, Flanger, Filter, Trans, Roll, Hpf, Lpf, Puff }

impl FxKind {
    pub fn name(self) -> &'static str {
        match self {
            FxKind::Echo => "ECHO", FxKind::Delay => "DELAY", FxKind::PingPong => "PING PONG", FxKind::Reverb => "REVERB", FxKind::Flanger => "FLANGER",
            FxKind::Filter => "FILTER", FxKind::Trans => "TRANS", FxKind::Roll => "ROLL", FxKind::Hpf => "HPF", FxKind::Lpf => "LPF", FxKind::Puff => "PUFF",
        }
    }
    pub fn from_name(s: &str) -> Option<FxKind> { FX_LIST.iter().map(|x| x.0).find(|k| k.name() == s) }
}

/// where the beat is at time `t0` (audio clock, seconds) and how fast it moves
#[derive(Clone, Copy, Debug)]
pub struct FxClock { pub t0: f64, pub beat0: f64, pub bps: f64, pub spb: f64 }
impl Default for FxClock { fn default() -> Self { FxClock { t0: 0.0, beat0: 0.0, bps: 0.0, spb: 0.5 } } }

/// the beat choices each effect offers (None = no BEAT setting), its default beat and depth
pub struct FxSpec { pub beats: Option<&'static [f64]>, pub beat: f64, pub depth: f64 }
const SHORT: &[f64] = &[1.0 / 16.0, 1.0 / 8.0, 1.0 / 4.0, 1.0 / 2.0, 3.0 / 4.0, 1.0, 2.0, 4.0];
const LONG: &[f64] = &[1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
pub const FX_LIST: [(FxKind, FxSpec); 11] = [
    (FxKind::Echo, FxSpec { beats: Some(SHORT), beat: 0.5, depth: 0.55 }),
    (FxKind::Delay, FxSpec { beats: Some(SHORT), beat: 0.75, depth: 0.6 }),
    (FxKind::PingPong, FxSpec { beats: Some(SHORT), beat: 0.5, depth: 0.6 }),
    (FxKind::Reverb, FxSpec { beats: None, beat: 1.0, depth: 0.55 }),
    (FxKind::Flanger, FxSpec { beats: Some(LONG), beat: 16.0, depth: 0.8 }),
    (FxKind::Filter, FxSpec { beats: Some(LONG), beat: 4.0, depth: 0.7 }),
    (FxKind::Trans, FxSpec { beats: Some(&[1.0 / 16.0, 1.0 / 8.0, 1.0 / 4.0, 1.0 / 2.0, 1.0, 2.0]), beat: 0.25, depth: 1.0 }),
    (FxKind::Roll, FxSpec { beats: Some(&[1.0 / 16.0, 1.0 / 8.0, 1.0 / 4.0, 1.0 / 2.0, 1.0, 2.0, 4.0]), beat: 0.5, depth: 1.0 }),
    (FxKind::Hpf, FxSpec { beats: None, beat: 1.0, depth: 0.6 }),
    (FxKind::Lpf, FxSpec { beats: None, beat: 1.0, depth: 0.6 }),
    (FxKind::Puff, FxSpec { beats: None, beat: 1.0, depth: 0.7 }),
];
pub fn spec(k: FxKind) -> &'static FxSpec { &FX_LIST.iter().find(|x| x.0 == k).unwrap().1 }
/// PAD FX banks (FX1, FX2): what each pad does (effect, beats, depth)
pub const PAD_FX: [[(FxKind, f64, f64); 8]; 2] = [
    [(FxKind::Echo, 0.5, 0.6), (FxKind::Echo, 1.0, 0.6), (FxKind::Reverb, 1.0, 0.7), (FxKind::Roll, 0.25, 1.0), (FxKind::Roll, 0.5, 1.0), (FxKind::Trans, 0.25, 1.0), (FxKind::Hpf, 1.0, 0.7), (FxKind::Lpf, 1.0, 0.7)],
    [(FxKind::PingPong, 0.5, 0.6), (FxKind::Delay, 0.75, 0.6), (FxKind::Flanger, 16.0, 0.8), (FxKind::Roll, 0.125, 1.0), (FxKind::Roll, 1.0, 1.0), (FxKind::Trans, 0.125, 1.0), (FxKind::Filter, 4.0, 0.8), (FxKind::Echo, 0.25, 0.7)],
];
/// 1/16, 3/4, 2 ...
pub fn frac_text(b: f64) -> String {
    if b >= 1.0 { if b.fract() == 0.0 { format!("{}", b as i64) } else { format!("{b}") } } else if b == 0.75 { "3/4".into() } else { format!("1/{}", js_round(1.0 / b) as i64) }
}

/// seconds of input history (roll) and of each delay line
const FX_HIST: f64 = 5.0;
#[inline]
fn js_round(x: f64) -> f64 { (x + 0.5).floor() }

struct Reverb { cl: Vec<Vec<f32>>, cr: Vec<Vec<f32>>, ci: [usize; 8], fsl: [f64; 8], fsr: [f64; 8], al: Vec<Vec<f32>>, ar: Vec<Vec<f32>>, ai: [usize; 4] }
impl Reverb {
    fn new(sr: f64) -> Reverb {
        let k = sr / 44100.0;
        let mk = |n: usize| vec![0f32; (js_round(n as f64 * k) as usize).max(8)];
        let combs = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let aps = [556, 441, 341, 225];
        Reverb {
            cl: combs.iter().map(|&n| mk(n)).collect(), cr: combs.iter().map(|&n| mk(n + 23)).collect(), ci: [0; 8], fsl: [0.0; 8], fsr: [0.0; 8],
            al: aps.iter().map(|&n| mk(n)).collect(), ar: aps.iter().map(|&n| mk(n + 23)).collect(), ai: [0; 4],
        }
    }
}

#[derive(Clone, Copy)]
struct RollState { start: usize, len: usize, pos: usize }

pub struct FxSlot {
    sr: f64,
    pub fx: Option<FxKind>,
    pub on: bool,
    pub beats: f64,
    pub depth: f64,
    /// PUFF: how hard the Puffco is being hit right now, 0..1 (set from outside, smoothed here)
    pub modv: f64,
    mod_s: f64,
    lfo: f64,
    g: f64,
    in_g: f64,
    /// on, or still ringing out
    pub active: bool,
    dl: Vec<f32>,
    dr: Vec<f32>,
    w: usize,
    fl: [f64; 4],
    fr: [f64; 4],
    cf: [f64; 5],
    cf_ready: bool,
    rev: Reverb,
    roll: Option<RollState>,
    quiet: usize,
    tg: Option<f64>,
}

/// a slot "set" message: None leaves a setting as it is
#[derive(Clone, Copy, Debug, Default)]
pub struct SlotSet { pub fx: Option<FxKind>, pub beats: Option<f64>, pub depth: Option<f64>, pub on: Option<bool> }

impl FxSlot {
    fn new(sr: f64) -> FxSlot {
        let n = (FX_HIST * sr).ceil() as usize;
        FxSlot {
            sr, fx: None, on: false, beats: 0.5, depth: 0.6, modv: 0.0, mod_s: 0.0, lfo: 0.0, g: 0.0, in_g: 0.0, active: false,
            dl: vec![0.0; n], dr: vec![0.0; n], w: 0, fl: [0.0; 4], fr: [0.0; 4], cf: [0.0; 5], cf_ready: false, rev: Reverb::new(sr), roll: None, quiet: 0, tg: None,
        }
    }
    pub fn set(&mut self, s: SlotSet) {
        if let Some(f) = s.fx { if Some(f) != self.fx { self.fx = Some(f); self.roll = None; self.cf_ready = false; } }
        if let Some(b) = s.beats { self.beats = b; }
        if let Some(d) = s.depth { self.depth = d; }
        if let Some(o) = s.on { if o && !self.on { self.roll = None; } self.on = o; }
        if self.on { self.active = true; self.quiet = 0; }
    }
    pub fn set_mod(&mut self, m: f64) { self.modv = m.clamp(0.0, 1.0); }
}

/// RBJ biquad coefficients (b0, b1, b2, a1, a2)
fn biq(hp: bool, f: f64, q: f64, sr: f64) -> [f64; 5] {
    let w = 2.0 * PI * f.min(sr * 0.45) / sr;
    let (cs, al) = (w.cos(), w.sin() / (2.0 * q));
    let (b0, b1, b2) = if hp { ((1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0) } else { ((1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0) };
    let a0 = 1.0 + al;
    [b0 / a0, b1 / a0, b2 / a0, (-2.0 * cs) / a0, (1.0 - al) / a0]
}

pub struct FxUnit {
    sr: f64,
    hl: Vec<f32>,
    hr: Vec<f32>,
    hw: usize,
    hn: usize,
    pub beat: FxSlot,
    pub pad: FxSlot,
    pub clock: FxClock,
}

impl FxUnit {
    pub fn new(sr: f64) -> FxUnit {
        let hn = (FX_HIST * sr).ceil() as usize;
        FxUnit { sr, hl: vec![0.0; hn], hr: vec![0.0; hn], hw: 0, hn, beat: FxSlot::new(sr), pad: FxSlot::new(sr), clock: FxClock::default() }
    }
    pub fn active(&self) -> bool { self.beat.active || self.pad.active }
    /// a silent block went past (the channel is idle): keep the history's timeline going without the work
    pub fn idle(&mut self, n: usize) {
        for _ in 0..n { self.hl[self.hw] = 0.0; self.hr[self.hw] = 0.0; self.hw += 1; if self.hw == self.hn { self.hw = 0; } }
    }
    /// in place; `time` is the audio clock at the block's start
    pub fn process(&mut self, ol: &mut [f32], or: &mut [f32], time: f64) {
        let n = ol.len();
        let mut hw = self.hw;
        for i in 0..n {
            self.hl[hw] = ol[i]; self.hr[hw] = or[i];
            hw += 1; if hw == self.hn { hw = 0; }
        }
        self.hw = hw;
        if !self.beat.active && !self.pad.active { return; }
        let c = self.clock;
        let beat_at0 = c.beat0 + (time - c.t0) * c.bps;
        let d_beat = c.bps / self.sr;
        let (hl, hr, hn) = (&self.hl, &self.hr, self.hn);
        if self.beat.active { run(&mut self.beat, ol, or, beat_at0, d_beat, c.spb, hw, hl, hr, hn); }
        if self.pad.active { run(&mut self.pad, ol, or, beat_at0, d_beat, c.spb, hw, hl, hr, hn); }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(s: &mut FxSlot, ol: &mut [f32], or: &mut [f32], beat0: f64, d_beat: f64, spb: f64, hist_end: usize, hl: &[f32], hr: &[f32], hn: usize) {
    let Some(fx) = s.fx else { s.active = false; return };
    let nn = ol.len();
    let sr = s.sr;
    let (on, depth) = (s.on, s.depth);
    let g_step = 1.0 / (0.004 * sr); // 4 ms fades in and out: no clicks
    let (mut g, mut in_g, mut peak) = (s.g, s.in_g, 0.0f64);
    let delay_family = matches!(fx, FxKind::Echo | FxKind::Delay | FxKind::PingPong);
    let dn = s.dl.len();
    let fb = match fx { FxKind::Echo => 0.6, FxKind::PingPong => 0.55, _ => 0.32 };
    let d_samp = if delay_family { (js_round(s.beats * spb * sr) as i64).clamp(1, dn as i64 - 2) as usize } else { 0 };
    let mut w = s.w;
    for i in 0..nn {
        let (xl, xr) = (ol[i] as f64, or[i] as f64);
        let want = if on { 1.0 } else { 0.0 };
        g += if g < want { g_step.min(want - g) } else { -g_step.min(g - want) };
        in_g += if in_g < want { g_step.min(want - in_g) } else { -g_step.min(in_g - want) };
        let (yl, yr);
        if delay_family {
            // echoes: the dry signal always passes; switching off stops feeding the delay, so they ring out
            let r = if w >= d_samp { w - d_samp } else { w + dn - d_samp };
            let (dl, dr) = (s.dl[r] as f64, s.dr[r] as f64);
            if fx == FxKind::PingPong { s.dl[w] = ((xl + xr) * 0.5 * in_g + dr * fb) as f32; s.dr[w] = (dl * fb) as f32; }
            else { s.dl[w] = (xl * in_g + dl * fb) as f32; s.dr[w] = (xr * in_g + dr * fb) as f32; }
            yl = xl + dl * depth; yr = xr + dr * depth;
            let a = dl.abs() + dr.abs(); if a > peak { peak = a; }
            w += 1; if w == dn { w = 0; }
        } else if fx == FxKind::Reverb {
            // Freeverb: 8 damped combs into 4 allpasses per side; switching off lets the room ring out
            let in_s = (xl + xr) * 0.015 * in_g;
            let rv = &mut s.rev;
            let (mut sl, mut sr2) = (0.0, 0.0);
            for k in 0..8 {
                let j = rv.ci[k];
                let (jl, jr) = (j % rv.cl[k].len(), j % rv.cr[k].len());
                let (o_l, o_r) = (rv.cl[k][jl] as f64, rv.cr[k][jr] as f64);
                rv.fsl[k] = o_l * 0.8 + rv.fsl[k] * 0.2; rv.fsr[k] = o_r * 0.8 + rv.fsr[k] * 0.2;
                rv.cl[k][jl] = (in_s + rv.fsl[k] * 0.86) as f32; rv.cr[k][jr] = (in_s + rv.fsr[k] * 0.86) as f32;
                sl += o_l; sr2 += o_r;
                rv.ci[k] = if j + 1 >= 1_000_000_000 { 0 } else { j + 1 };
            }
            for k in 0..4 {
                let j = rv.ai[k];
                let (jl, jr) = (j % rv.al[k].len(), j % rv.ar[k].len());
                let (bo, bo2) = (rv.al[k][jl] as f64, rv.ar[k][jr] as f64);
                rv.al[k][jl] = (sl + bo * 0.5) as f32; rv.ar[k][jr] = (sr2 + bo2 * 0.5) as f32;
                sl = bo - sl; sr2 = bo2 - sr2;
                rv.ai[k] = if j + 1 >= 1_000_000_000 { 0 } else { j + 1 };
            }
            yl = xl + sl * depth * 1.2; yr = xr + sr2 * depth * 1.2;
            let a = sl.abs() + sr2.abs(); if a > peak { peak = a; }
        } else {
            let beat = beat0 + i as f64 * d_beat;
            let (mut el, mut er) = (xl, xr); // the fully-wet signal
            match fx {
                FxKind::Flanger => {
                    // a 1-5 ms delay swept once per cycle (BEAT sets the cycle length), with feedback
                    let cyc = s.beats.max(0.25);
                    let ph = beat / cyc;
                    let dd = (0.001 + 0.004 * (0.5 - 0.5 * (2.0 * PI * ph).cos())) * sr;
                    let mut rp = w as f64 - dd; if rp < 0.0 { rp += dn as f64; }
                    let i0 = rp as usize; let f = rp - i0 as f64; let i1 = if i0 + 1 == dn { 0 } else { i0 + 1 };
                    let dl = s.dl[i0] as f64 + (s.dl[i1] as f64 - s.dl[i0] as f64) * f;
                    let dr = s.dr[i0] as f64 + (s.dr[i1] as f64 - s.dr[i0] as f64) * f;
                    s.dl[w] = (xl + dl * 0.6) as f32; s.dr[w] = (xr + dr * 0.6) as f32;
                    w += 1; if w == dn { w = 0; }
                    el = (xl + dl * depth) / (1.0 + 0.6 * depth); er = (xr + dr * depth) / (1.0 + 0.6 * depth);
                }
                FxKind::Puff => {
                    // tape warble: a sine-swept 1-8 ms delay (pitch wobble, no dry mix); the hit sets both how far
                    // and how fast it sweeps (2 Hz light pull ... 7 Hz hard rip), LEVEL/DEPTH caps it
                    s.mod_s += (s.modv - s.mod_s) * (1.0 / (0.05 * sr)); // ~50 ms: BLE readings arrive in steps
                    let a = s.mod_s * depth;
                    s.lfo += (2.0 + 5.0 * a) / sr; if s.lfo >= 1.0 { s.lfo -= 1.0; }
                    let dd = (0.0045 + 0.0035 * a * (2.0 * PI * s.lfo).sin()) * sr;
                    let mut rp = w as f64 - dd; if rp < 0.0 { rp += dn as f64; }
                    let i0 = rp as usize; let f = rp - i0 as f64; let i1 = if i0 + 1 == dn { 0 } else { i0 + 1 };
                    s.dl[w] = xl as f32; s.dr[w] = xr as f32;
                    el = s.dl[i0] as f64 + (s.dl[i1] as f64 - s.dl[i0] as f64) * f;
                    er = s.dr[i0] as f64 + (s.dr[i1] as f64 - s.dr[i0] as f64) * f;
                    w += 1; if w == dn { w = 0; }
                }
                FxKind::Filter | FxKind::Hpf | FxKind::Lpf => {
                    if (i & 31) == 0 || !s.cf_ready {
                        s.cf = match fx {
                            // LFO low-pass sweep, one cycle per BEAT setting; LEVEL/DEPTH sets how low it dips
                            FxKind::Filter => { let ph = beat / s.beats.max(0.25); biq(false, 20000.0 * 2f64.powf(-9.0 * depth * (0.5 - 0.5 * (2.0 * PI * ph).cos())), 2.2, sr) }
                            FxKind::Hpf => biq(true, 20.0 * 2f64.powf(9.5 * depth), 1.1, sr),
                            _ => biq(false, 20000.0 * 2f64.powf(-9.5 * depth), 1.1, sr),
                        };
                        s.cf_ready = true;
                    }
                    let c = s.cf;
                    el = c[0] * xl + c[1] * s.fl[0] + c[2] * s.fl[1] - c[3] * s.fl[2] - c[4] * s.fl[3];
                    s.fl[1] = s.fl[0]; s.fl[0] = xl; s.fl[3] = s.fl[2]; s.fl[2] = el;
                    er = c[0] * xr + c[1] * s.fr[0] + c[2] * s.fr[1] - c[3] * s.fr[2] - c[4] * s.fr[3];
                    s.fr[1] = s.fr[0]; s.fr[0] = xr; s.fr[3] = s.fr[2]; s.fr[2] = er;
                }
                FxKind::Trans => {
                    // gate: on for the first half of each BEAT-long step, cut (by LEVEL/DEPTH) for the second half
                    let st = s.beats.max(1.0 / 32.0);
                    let ph = beat / st;
                    let frc = ph - ph.floor();
                    let want2 = if frc < 0.5 { 1.0 } else { 1.0 - depth };
                    let tg = match s.tg { None => want2, Some(t) => t + (want2 - t) * 0.02 }; // ~1 ms smoothing
                    s.tg = Some(tg);
                    el = xl * tg; er = xr * tg;
                }
                FxKind::Roll => {
                    // repeat the last BEAT-long slice, lined up with the grid (it starts on the slice the playhead is in)
                    if s.roll.is_none() && on {
                        let len = (js_round(s.beats * spb * sr) as i64).clamp(64, hn as i64 - 256) as usize;
                        let st = s.beats.max(1.0 / 32.0);
                        let frc = beat / st - (beat / st).floor();
                        let into = js_round(frc * len as f64) as usize; // samples since the slice began
                        let now_idx = hist_end as i64 - nn as i64 + i as i64;
                        let mut start = now_idx - into as i64;
                        while start < 0 { start += hn as i64; }
                        s.roll = Some(RollState { start: start as usize, len, pos: into });
                    }
                    if let Some(r) = s.roll.as_mut() {
                        let mut j = r.start + (r.pos % r.len); if j >= hn { j -= hn; }
                        el = hl[j] as f64; er = hr[j] as f64; r.pos += 1;
                    }
                    if !on && g < 0.001 { s.roll = None; }
                }
                _ => {}
            }
            let mix = if fx == FxKind::Roll { depth.max(0.001) } else { 1.0 };
            yl = xl + (el - xl) * g * mix; yr = xr + (er - xr) * g * mix;
        }
        ol[i] = yl as f32; or[i] = yr as f32;
    }
    s.g = g; s.in_g = in_g; s.w = w;
    // done ringing out? (off, faded, and the delay lines / reverb have gone quiet for half a second)
    if !on && g < 0.0005 && in_g < 0.0005 {
        if delay_family || fx == FxKind::Reverb {
            if peak < 1e-4 { s.quiet += nn; } else { s.quiet = 0; }
            if s.quiet as f64 > 0.5 * sr { s.active = false; }
        } else { s.active = false; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(n: usize, sr: f64) -> Vec<f32> { (0..n).map(|i| (0.5 * (2.0 * PI * 440.0 * i as f64 / sr).sin()) as f32).collect() }
    #[test]
    fn off_is_a_straight_copy_and_echo_rings_out() {
        let sr = 48000.0;
        let mut u = FxUnit::new(sr);
        let x = tone(4800, sr);
        let (mut l, mut r) = (x.clone(), x.clone());
        u.process(&mut l, &mut r, 0.0);
        assert_eq!(l, x);
        u.clock = FxClock { t0: 0.0, beat0: 0.0, bps: 2.0, spb: 0.5 };
        u.beat.set(SlotSet { fx: Some(FxKind::Echo), beats: Some(0.5), depth: Some(0.6), on: Some(true) });
        for _ in 0..10 { let (mut l, mut r) = (x.clone(), x.clone()); u.process(&mut l, &mut r, 0.0); }
        u.beat.set(SlotSet { on: Some(false), ..Default::default() });
        // silence in: the echo tail still comes out
        let (mut l, mut r) = (vec![0f32; 4800], vec![0f32; 4800]);
        u.process(&mut l, &mut r, 1.0);
        assert!(l.iter().any(|v| v.abs() > 0.05), "echo tail");
        // and it stops being active once the tail has died away
        for k in 0..400 { let (mut l, mut r) = (vec![0f32; 480], vec![0f32; 480]); u.process(&mut l, &mut r, 2.0 + k as f64 * 0.01); }
        assert!(!u.active());
    }
    #[test]
    fn trans_gates_on_the_beat() {
        let sr = 48000.0;
        let mut u = FxUnit::new(sr);
        u.clock = FxClock { t0: 0.0, beat0: 0.0, bps: 2.0, spb: 0.5 };
        u.pad.set(SlotSet { fx: Some(FxKind::Trans), beats: Some(1.0), depth: Some(1.0), on: Some(true) });
        let (mut l, mut r) = (vec![1f32; 48000], vec![1f32; 48000]);
        u.process(&mut l, &mut r, 0.0);
        // beat = 0.5 s: open for the first 0.25 s, shut for the next
        assert!(l[6000] > 0.99 && l[18000] < 0.01 && l[30000] > 0.99, "{} {} {}", l[6000], l[18000], l[30000]);
    }
}
