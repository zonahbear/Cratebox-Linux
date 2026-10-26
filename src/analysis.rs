//! Track analysis: waveform bands, beat grid (BPM, phase, downbeat), musical key and phrases.
//! A line-by-line port of the web version's analyser, so both give the same results.

use crate::fft::{hann, Fft};

/// JavaScript's Math.round (halves go up), so results match the web version exactly
#[inline]
pub fn jsround(x: f64) -> f64 { (x + 0.5).floor() }

pub const ANALYSIS_VER: u32 = 3;

/// mono, downsampled by an integer factor to about `target` Hz (averaging), from planar channels
pub fn mono_for(chans: &[&[f32]], sr: u32, target: f64) -> (Vec<f32>, f64) {
    let f = ((sr as f64 / target).round() as usize).max(1);
    let len = chans.iter().map(|c| c.len()).min().unwrap_or(0);
    let n = len / f;
    let use_ch = &chans[..chans.len().min(2)];
    let k = 1.0 / (f * use_ch.len()) as f32;
    let mut out = vec![0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let mut s = 0f32;
        for ch in use_ch { for j in 0..f { s += ch[i * f + j]; } }
        *o = s * k;
    }
    (out, sr as f64 / f as f64)
}

#[derive(Clone, Debug, Default)]
pub struct Wave {
    pub duration: f64,
    pub wps: f64,
    pub peak: Vec<u8>,
    pub rms: Vec<u8>,
    pub low: Vec<u8>,
    pub mid: Vec<u8>,
    pub high: Vec<u8>,
}

/// Waveform at 300 points per second from a ~22 kHz mono: overall peak and RMS (the shape) plus the
/// RMS of three bands (the colour: lows < 200 Hz, mids, highs > 2.5 kHz), each stored as bytes.
pub fn waveform(x: &[f32], fs: f64) -> Wave {
    let n = x.len();
    let wps = 300.0;
    let hop = fs / wps;
    let frames = (n as f64 / hop).ceil() as usize;
    let (mut pk, mut rm, mut lo, mut mi, mut hi) = (vec![0f32; frames], vec![0f32; frames], vec![0f32; frames], vec![0f32; frames], vec![0f32; frames]);
    let coef = |lp: bool, f0: f64| -> [f64; 5] {
        let w = 2.0 * std::f64::consts::PI * f0 / fs;
        let c = w.cos();
        let al = w.sin() / (2.0 * 0.7071);
        let a0 = 1.0 + al;
        let b = if lp { [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0] } else { [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0] };
        [b[0] / a0, b[1] / a0, b[2] / a0, -2.0 * c / a0, (1.0 - al) / a0]
    };
    let l = coef(true, 200.0);
    let h = coef(false, 2500.0);
    let (mut l1a, mut l1b, mut l2a, mut l2b, mut h1a, mut h1b, mut h2a, mut h2b) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    let mut f = 0usize;
    let mut next = hop;
    let (mut sp, mut sr, mut sl, mut sm, mut sh, mut cnt) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for i in 0..n {
        let s = x[i] as f64;
        let mut y = l[0] * s + l1a; l1a = l[1] * s - l[3] * y + l1b; l1b = l[2] * s - l[4] * y;
        let low = l[0] * y + l2a; l2a = l[1] * y - l[3] * low + l2b; l2b = l[2] * y - l[4] * low;
        y = h[0] * s + h1a; h1a = h[1] * s - h[3] * y + h1b; h1b = h[2] * s - h[4] * y;
        let high = h[0] * y + h2a; h2a = h[1] * y - h[3] * high + h2b; h2b = h[2] * y - h[4] * high;
        let mid = s - low - high;
        let a = s.abs();
        if a > sp { sp = a; }
        sr += s * s; sl += low * low; sm += mid * mid; sh += high * high; cnt += 1.0;
        if (i + 1) as f64 >= next || i == n - 1 {
            if f < frames {
                pk[f] = sp as f32; rm[f] = (sr / cnt).sqrt() as f32; lo[f] = (sl / cnt).sqrt() as f32; mi[f] = (sm / cnt).sqrt() as f32; hi[f] = (sh / cnt).sqrt() as f32;
            }
            f += 1; next += hop; sp = 0.0; sr = 0.0; sl = 0.0; sm = 0.0; sh = 0.0; cnt = 0.0;
        }
    }
    let f = f.min(frames);
    let top = |arr: &[f32], q: f64| -> f64 {
        let mut t: Vec<f32> = arr[..f].to_vec();
        t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if t.is_empty() { return 1e-9; }
        let v = t[((t.len() as f64 * q).floor() as usize).min(t.len() - 1)] as f64;
        if v == 0.0 { 1e-9 } else { v }
    };
    let bytes = |arr: &[f32], r: f64, gamma: f64| -> Vec<u8> {
        (0..f).map(|i| (255.0 * (arr[i] as f64 / r).powf(gamma)).round().min(255.0) as u8).collect()
    };
    let pref = top(&pk, 0.999);
    Wave {
        duration: n as f64 / fs, wps,
        peak: bytes(&pk, pref, 1.0), rms: bytes(&rm, pref, 1.0),
        low: bytes(&lo, top(&lo, 0.995), 0.9), mid: bytes(&mi, top(&mi, 0.995), 0.9), high: bytes(&hi, top(&hi, 0.995), 0.9),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub start: f64,
    pub bars: u32,
    pub label: String,
}

pub const PHRASE_LABELS: [&str; 6] = ["Intro", "Build", "Drop", "Breakdown", "Groove", "Outro"];
pub fn phrase_color(label: &str) -> [u8; 3] {
    match label {
        "Intro" => [0x4c, 0xc9, 0xf0], "Build" => [0xff, 0xd2, 0x3f], "Drop" => [0xff, 0x4d, 0x6d],
        "Breakdown" => [0x9b, 0x7b, 0xff], "Groove" => [0x3d, 0xdc, 0x84], "Outro" => [0x5b, 0x7c, 0xfa], _ => [0x88, 0x88, 0x88],
    }
}

fn pct(v: &[f64], q: f64) -> f64 {
    if v.is_empty() { return 0.0; }
    let mut t = v.to_vec();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    t[((q * t.len() as f64).floor() as usize).min(t.len() - 1)]
}

/// Per bar: loudness, low end, highs, and how much the low end pulses on the beats (kick drum).
/// Boundaries go where those change most, on 4-bar steps from bar 1; each section is then named
/// from its level relative to the rest of the track, whether the kick is in, and what comes next.
pub fn phrase_analysis(x: &[f32], fs: f64, bpm: f64, first_beat: f64) -> Vec<Phrase> {
    let n = x.len();
    if !(bpm > 0.0) { return vec![]; }
    let spb = 60.0 / bpm;
    let bar_len = 4.0 * spb;
    let nb_f = ((n as f64 / fs - first_beat) / bar_len).floor();
    if !(nb_f >= 8.0) { return vec![]; }
    let n_bars = nb_f as usize;
    let hop = jsround(fs / 200.0) as usize;
    let er = fs / hop as f64;
    let m = n / hop;
    let (mut e_all, mut e_low, mut e_high, mut e_mid) = (vec![0f32; m], vec![0f32; m], vec![0f32; m], vec![0f32; m]);
    let al = 1.0 - (-2.0 * std::f64::consts::PI * 150.0 / fs).exp();
    let ah = 1.0 - (-2.0 * std::f64::consts::PI * 2500.0 / fs).exp();
    let (mut l1, mut l2, mut h1) = (0f64, 0f64, 0f64);
    for j in 0..m {
        let (mut sa, mut sl, mut sh, mut sm) = (0f64, 0f64, 0f64, 0f64);
        for i in j * hop..j * hop + hop {
            let s = x[i] as f64;
            l1 += al * (s - l1); l2 += al * (l1 - l2);
            h1 += ah * (s - h1);
            let hi = s - h1;
            let md = h1 - l2;
            sa += s * s; sl += l2 * l2; sh += hi * hi; sm += md * md;
        }
        let hf = hop as f64;
        e_all[j] = (sa / hf).sqrt() as f32; e_low[j] = (sl / hf).sqrt() as f32; e_high[j] = (sh / hf).sqrt() as f32; e_mid[j] = (sm / hf).sqrt() as f32;
    }
    let db = |v: f64| 20.0 * (v + 1e-6).log10();
    struct F { e: f64, low: f64, high: f64, mid: f64, kick: f64 }
    let mut fv: Vec<F> = Vec::with_capacity(n_bars);
    let get = |arr: &[f32], i: i64| -> Option<f64> { if i >= 0 && (i as usize) < arr.len() { Some(arr[i as usize] as f64) } else { None } };
    for b in 0..n_bars {
        let t0 = first_beat + b as f64 * bar_len;
        let j0 = (t0 * er).floor() as usize;
        let j1 = ((t0 + bar_len) * er).floor() as usize;
        let (mut sa, mut sl, mut sh, mut sm, mut c) = (0f64, 0f64, 0f64, 0f64, 0f64);
        let mut j = j0;
        while j < j1 && j < m {
            sa += (e_all[j] as f64).powi(2); sl += (e_low[j] as f64).powi(2); sh += (e_high[j] as f64).powi(2); sm += (e_mid[j] as f64).powi(2); c += 1.0;
            j += 1;
        }
        let mut rises = Vec::with_capacity(16);
        for k in 0..16 {
            let jb = ((t0 + k as f64 * spb / 4.0) * er).floor() as i64;
            let mut before = f64::INFINITY;
            let mut after = 0f64;
            for q in -6..0 { if let Some(v) = get(&e_low, jb + q) { if v < before { before = v; } } }
            for q in 0..6 { if let Some(v) = get(&e_low, jb + q) { if v > after { after = v; } } }
            rises.push(if before != f64::INFINITY { (after - before).max(0.0) } else { 0.0 });
        }
        rises.sort_by(|p, q| q.partial_cmp(p).unwrap());
        let cc = if c > 0.0 { c } else { 1.0 };
        fv.push(F { e: db((sa / cc).sqrt()), low: db((sl / cc).sqrt()), high: db((sh / cc).sqrt()), mid: db((sm / cc).sqrt()), kick: (rises[0] + rises[1]) / 2.0 });
    }
    let es: Vec<f64> = fv.iter().map(|f| f.e).collect();
    let e_hi = pct(&es, 0.85);
    let e_lo = pct(&es, 0.15);
    let low_hi = pct(&fv.iter().map(|f| f.low).collect::<Vec<_>>(), 0.85);
    let mut k_ref = pct(&fv.iter().map(|f| f.kick).collect::<Vec<_>>(), 0.9);
    if k_ref == 0.0 { k_ref = 1e-6; }
    for f in fv.iter_mut() { f.kick /= k_ref; }
    #[derive(Clone, Copy)]
    enum K { E, Low, High, Mid, Kick }
    let mean = |a: i64, b: i64, key: K| -> f64 {
        let (mut s, mut c) = (0f64, 0f64);
        let lo = a.max(0);
        let hi = b.min(n_bars as i64);
        let mut i = lo;
        while i < hi {
            let f = &fv[i as usize];
            s += match key { K::E => f.e, K::Low => f.low, K::High => f.high, K::Mid => f.mid, K::Kick => f.kick };
            c += 1.0; i += 1;
        }
        if c > 0.0 { s / c } else { 0.0 }
    };
    let kick_on = |v: f64| ((v - 0.2) / 0.3).clamp(0.0, 1.0);
    let mut nov: Vec<(i64, f64)> = vec![];
    let nb = n_bars as i64;
    let mut b = 4i64;
    while b < nb - 2 {
        let w = 4;
        let d = (mean(b, b + w, K::E) - mean(b - w, b, K::E)).abs() / 4.0
            + (mean(b, b + w, K::Low) - mean(b - w, b, K::Low)).abs() / 5.0
            + (mean(b, b + w, K::High) - mean(b - w, b, K::High)).abs() / 6.0
            + (mean(b, b + w, K::Mid) - mean(b - w, b, K::Mid)).abs() / 5.0
            + 1.5 * (kick_on(mean(b, b + w, K::Kick)) - kick_on(mean(b - w, b, K::Kick))).abs();
        nov.push((b, d));
        b += 4;
    }
    let mut cuts: Vec<i64> = vec![0];
    let d_med = pct(&nov.iter().map(|v| v.1).collect::<Vec<_>>(), 0.5);
    let thr = (3.0 * d_med).max(0.45);
    let mut cand: Vec<(i64, f64)> = nov.iter().enumerate()
        .filter(|(i, v)| v.1 > thr && (*i == 0 || v.1 >= nov[i - 1].1) && (*i == nov.len() - 1 || v.1 >= nov[i + 1].1))
        .map(|(_, v)| *v).collect();
    cand.sort_by(|p, q| q.1.partial_cmp(&p.1).unwrap());
    for c in cand {
        if cuts.iter().all(|&x| (x - c.0).abs() >= 8 || (c.1 > 4.0 * thr && (x - c.0).abs() >= 4)) { cuts.push(c.0); }
    }
    cuts.sort();
    cuts.push(nb);
    struct Seg { a: i64, b: i64, level: f64, kick: f64, low: f64, rise: f64, label: &'static str }
    let mut segs: Vec<Seg> = vec![];
    for i in 0..cuts.len() - 1 {
        let (a, b) = (cuts[i], cuts[i + 1]);
        let e = mean(a, b, K::E);
        let kick = kick_on(mean(a, b, K::Kick));
        let low = mean(a, b, K::Low);
        let level = (e - e_lo) / (e_hi - e_lo).max(3.0);
        let rise = if b - a >= 4 { (mean(b - 2, b, K::E) - mean(a, a + 2, K::E)) + (mean(b - 2, b, K::High) - mean(a, a + 2, K::High)) * 0.5 } else { 0.0 };
        segs.push(Seg { a, b, level, kick, low, rise, label: "" });
    }
    for s in segs.iter_mut() {
        let full = s.kick > 0.5 && s.low > low_hi - 4.0;
        s.label = if full && s.level > 0.78 { "Drop" } else if s.kick > 0.5 { "Groove" } else { "Breakdown" };
    }
    // a long quiet section that ramps up into a drop: its last 8 bars are the build
    let mut i = 0;
    while i + 1 < segs.len() {
        let (sa, sb, slabel) = (segs[i].a, segs[i].b, segs[i].label);
        if segs[i + 1].label != "Drop" || slabel == "Drop" || sb - sa < 16 { i += 1; continue; }
        let c = sb - 8;
        let ramp = (mean(c, sb, K::E) - mean(sa, c, K::E)) + 0.5 * (mean(c, sb, K::High) - mean(sa, c, K::High));
        if ramp > 3.0 {
            let (level, kick, low) = (segs[i].level, segs[i].kick, segs[i].low);
            segs.insert(i + 1, Seg { a: c, b: sb, level, kick, low, rise: ramp, label: "Build" });
            segs[i].b = c;
            i += 1;
        }
        i += 1;
    }
    for i in 0..segs.len() {
        let nx_drop = i + 1 < segs.len() && segs[i + 1].label == "Drop";
        let s = &segs[i];
        if s.label != "Drop" && nx_drop && (s.rise > 3.0 || (s.kick < 0.5 && s.b - s.a <= 8)) && s.b - s.a <= 16 { segs[i].label = "Build"; }
    }
    if segs.len() > 1 && (segs[0].label != "Drop" || segs[0].level < 0.95) { segs[0].label = "Intro"; }
    let ln = segs.len();
    if ln > 2 {
        let l = &segs[ln - 1];
        if l.label != "Drop" && l.level < 0.8 && l.b - l.a <= 32 { segs[ln - 1].label = "Outro"; }
    }
    let mut merged: Vec<(i64, i64, &str)> = vec![];
    for s in &segs {
        if let Some(p) = merged.last_mut() { if p.2 == s.label { p.1 = s.b; continue; } }
        merged.push((s.a, s.b, s.label));
    }
    let mut out = vec![];
    for (a0, b, label) in merged {
        let mut a = a0;
        while b - a > 32 { out.push(Phrase { start: first_beat + a as f64 * bar_len, bars: 16, label: label.to_string() }); a += 16; }
        out.push(Phrase { start: first_beat + a as f64 * bar_len, bars: (b - a) as u32, label: label.to_string() });
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub bpm: f64,
    pub first_beat: f64,
    pub conf: f64,
}

// downbeat vote weights (tuned on the synthetic test set)
const W_KS: f64 = 0.5;
const W_NOV: f64 = 20.0;
const W_START: f64 = 2.0;
const W_ENTRY: f64 = 0.0;

#[derive(Clone, Copy)]
struct Pk { f: f64, v: f64 }
fn peak_near(arr: &[f32], f: f64, w: f64, frames: usize) -> Option<Pk> {
    let lo = ((f - w).floor()).max(1.0);
    let hi = ((f + w).ceil()).min(frames as f64 - 2.0);
    if hi < lo { return None; }
    let (lo, hi) = (lo as i64, hi as i64);
    let mut bi: i64 = -1;
    let mut bv = 0f64;
    let mut i = lo;
    while i <= hi {
        let v = arr[i as usize] as f64;
        if v > bv { bv = v; bi = i; }
        i += 1;
    }
    if bi < 0 { return None; }
    let a = arr[(bi - 1) as usize] as f64;
    let c = arr[(bi + 1) as usize] as f64;
    let d = a - 2.0 * bv + c;
    Some(Pk { f: bi as f64 + if d < 0.0 { 0.5 * (a - c) / d } else { 0.0 }, v: bv })
}

/// 1. Onsets: log-magnitude spectral flux in three bands (kick/bass, body/snare, hats) at ~172 fps.
/// 2. Tempo candidates: autocorrelation of the onset curve, scored at 1, 2 and 4 beats; related tempos
///    are added and every candidate is checked against the music by folding the onsets onto its grid.
/// 3. Exact tempo and phase: a straight line through the onsets nearest each predicted beat.
/// 4. On-beat vs off-beat.  5. Downbeat vote.
pub fn beat_grid(x: &[f32], fs: f64, range: (f64, f64)) -> Grid {
    let n = x.len();
    const N: usize = 512;
    const HOP: usize = 64;
    let fps = fs / HOP as f64;
    let frames = if n >= N { (n - N) / HOP + 1 } else { 0 };
    if (frames as f64) < fps * 4.0 { return Grid { bpm: 120.0, first_beat: 0.0, conf: 0.0 }; }
    let win = hann(N);
    let bin_hz = fs / N as f64;
    let bin = |f: f64| (jsround(f / bin_hz) as usize).max(1);
    let (b0, b1, b2) = (bin(30.0), bin(150.0), bin(2000.0));
    let b3 = (N / 2 - 1).min(bin(5000.0));
    let mut f_low = vec![0f32; frames];
    let mut f_mid = vec![0f32; frames];
    let mut f_high = vec![0f32; frames];
    let mut rms = vec![0f32; frames];
    const NB: usize = 16;
    let mut band_of = vec![-1i8; N / 2];
    for k in b0..b2 {
        band_of[k] = ((NB as f64 * (k as f64 / b0 as f64).ln() / (b2 as f64 / b0 as f64).ln()).floor() as usize).min(NB - 1) as i8;
    }
    let mut spec = vec![0f32; frames * NB];
    let fft = Fft::new(N);
    let mut re = vec![0f64; N];
    let mut im = vec![0f64; N];
    let mut prev = vec![0f32; N / 2];
    let mut cur = vec![0f32; N / 2];
    for t in 0..frames {
        let s0 = t * HOP;
        let mut e = 0f64;
        for i in 0..N { let v = x[s0 + i] as f64; re[i] = v * win[i]; im[i] = 0.0; e += v * v; }
        rms[t] = (e / N as f64).sqrt() as f32;
        fft.run(&mut re, &mut im);
        let (mut lo, mut mi, mut hi) = (0f64, 0f64, 0f64);
        for k in b0..=b3 {
            let l = (1.0 + 100.0 * re[k].hypot(im[k])).ln() as f32;
            cur[k] = l;
            let d = (l - prev[k]) as f64;
            if d > 0.0 { if k < b1 { lo += d; } else if k < b2 { mi += d; } else { hi += d; } }
            let bb = band_of[k];
            if bb >= 0 { spec[t * NB + bb as usize] += l; }
        }
        f_low[t] = (lo / (b1 - b0) as f64) as f32; f_mid[t] = (mi / (b2 - b1) as f64) as f32; f_high[t] = (hi / (b3 - b2 + 1) as f64) as f32;
        std::mem::swap(&mut prev, &mut cur);
    }
    f_low[0] = 0.0; f_mid[0] = 0.0; f_high[0] = 0.0;
    let w = jsround(0.15 * fps) as i64;
    let clean = |a: &[f32]| -> Vec<f32> {
        let mut out = vec![0f32; frames];
        let mut s = 0f64;
        for t in 0..(frames as i64).min(w) { s += a[t as usize] as f64; }
        let mut tot = 0f64;
        for t in 0..frames as i64 {
            let hi_i = t + w;
            let lo_i = t - w - 1;
            if hi_i < frames as i64 { s += a[hi_i as usize] as f64; }
            if lo_i >= 0 { s -= a[lo_i as usize] as f64; }
            let cnt = ((frames as i64 - 1).min(hi_i) - (lo_i + 1).max(0) + 1) as f64;
            let v = a[t as usize] as f64 - s / cnt;
            out[t as usize] = if v > 0.0 { v as f32 } else { 0.0 };
            tot += out[t as usize] as f64;
        }
        let m = { let m = tot / frames as f64; if m == 0.0 { 1.0 } else { m } };
        for o in out.iter_mut() { *o = (*o as f64 / m) as f32; }
        out
    };
    let o_l = clean(&f_low);
    let o_m = clean(&f_mid);
    let o_h = clean(&f_high);
    let mut odf = vec![0f32; frames];
    let mut beat_odf = vec![0f32; frames];
    for t in 0..frames {
        odf[t] = o_l[t] + o_m[t] + 0.6 * o_h[t];
        beat_odf[t] = o_l[t] + 0.8 * o_m[t] + 0.15 * o_h[t];
    }

    // ---- tempo candidates from the autocorrelation
    let mean: f64 = odf.iter().map(|&v| v as f64).sum::<f64>() / frames as f64;
    let z: Vec<f32> = odf.iter().map(|&v| (v as f64 - mean) as f32).collect();
    let max_lag = (frames - 1).min((fps * 60.0 / 55.0 * 4.0).ceil() as usize + 2);
    let mut ac = vec![0f32; max_lag + 1];
    for lag in 1..=max_lag {
        let mut s = 0f64;
        for i in lag..frames { s += z[i] as f64 * z[i - lag] as f64; }
        ac[lag] = (s / (frames - lag) as f64) as f32;
    }
    let ac_at = |l: f64| -> f64 {
        let i = l.floor() as i64;
        let f = l - i as f64;
        if i < 1 || i + 2 > max_lag as i64 { return 0.0; }
        let i = i as usize;
        let a = ac[i] as f64 + (ac[i + 1] as f64 - ac[i] as f64) * f;
        a.max(ac[i - 1] as f64 * 0.9).max(ac[i + 2] as f64 * 0.9)
    };
    let ac_score = |b: f64| { let p = fps * 60.0 / b; ac_at(p) + ac_at(2.0 * p) + ac_at(4.0 * p) };
    let mut tempos: Vec<(f64, f64)> = vec![];
    let mut b = 60.0;
    while b <= 200.0 { tempos.push((b, ac_score(b))); b += 0.1; }
    let mut peaks: Vec<(f64, f64)> = vec![];
    for i in 1..tempos.len() - 1 { if tempos[i].1 >= tempos[i - 1].1 && tempos[i].1 > tempos[i + 1].1 { peaks.push(tempos[i]); } }
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let fold = |mut b: f64| { while b < range.0 { b *= 2.0; } while b >= range.1 { b /= 2.0; } b };
    let mut cands: Vec<f64> = vec![];
    let add_cand = |b: f64, cands: &mut Vec<f64>| { let b = fold(b); if !cands.iter().any(|c| (c / b - 1.0).abs() < 0.004) { cands.push(b); } };
    for p in peaks.iter().take(5) { add_cand(p.0, &mut cands); }
    for p in peaks.iter().take(2) { for r in [2.0 / 3.0, 3.0 / 2.0, 3.0 / 4.0, 4.0 / 3.0] { add_cand(p.0 * r, &mut cands); } }
    if cands.is_empty() { return Grid { bpm: 120.0, first_beat: 0.0, conf: 0.0 }; }

    // ---- grid quality for a tempo: fold onsets into a beat-phase histogram
    const HB: usize = 64;
    let mut hist = [0f64; HB];
    let mut fit = |b: f64| -> (f64, f64, f64) {
        let p = fps * 60.0 / b;
        hist.fill(0.0);
        for t in 0..frames {
            let v = beat_odf[t] as f64;
            if v <= 0.0 { continue; }
            let ph = ((t as f64 / p) % 1.0) * HB as f64;
            let i0 = ph.floor() as usize;
            let f = ph - i0 as f64;
            hist[i0 % HB] += v * (1.0 - f);
            hist[(i0 + 1) % HB] += v * f;
        }
        let (mut mx, mut mi, mut tot) = (-1f64, 0usize, 0f64);
        for k in 0..HB {
            let s = hist[k] + 0.5 * (hist[(k + 1) % HB] + hist[(k + HB - 1) % HB]);
            tot += hist[k];
            if s > mx { mx = s; mi = k; }
        }
        let den = 2.0 * tot / HB as f64;
        (mx / if den == 0.0 { 1.0 } else { den }, (mi as f64 / HB as f64) * p, p)
    };
    let odf_mean = { let m = beat_odf.iter().map(|&v| v as f64).sum::<f64>() / frames as f64; if m == 0.0 { 1.0 } else { m } };
    let at = |i: i64| -> f64 { if i >= 0 && (i as usize) < frames { beat_odf[i as usize] as f64 } else { 0.0 } };
    let contrast = |p: f64, ph: f64| -> f64 {
        let (mut s, mut c) = (0f64, 0f64);
        let mut f = ph;
        while f < frames as f64 - 3.0 {
            let i = jsround(f) as i64;
            s += at(i - 2).max(at(i - 1)).max(at(i)).max(at(i + 1)).max(at(i + 2));
            c += 1.0;
            f += p;
        }
        if c > 0.0 { s / c / odf_mean } else { 0.0 }
    };
    let ac_max = if peaks.is_empty() { 1.0 } else { peaks[0].1 }.max(1e-9);

    let mut sorted: Vec<f32> = beat_odf.iter().copied().filter(|&v| v > 0.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let thr = if sorted.is_empty() { 0.0 } else { sorted[(sorted.len() as f64 * 0.6).floor() as usize] as f64 };
    // exact tempo + phase: line through the onsets at each beat
    let regress = |p: &mut f64, a0: &mut f64, win_frac: f64| -> bool {
        let k0 = (-*a0 / *p).ceil() as i64;
        let k1 = ((frames as f64 - 1.0 - *a0) / *p).floor() as i64;
        let (mut ks, mut fs_, mut ws) = (vec![], vec![], vec![]);
        for k in k0..=k1 {
            if let Some(pk) = peak_near(&beat_odf, *a0 + k as f64 * *p, *p * win_frac, frames) {
                if pk.v < thr { continue; }
                ks.push(k as f64); fs_.push(pk.f); ws.push(pk.v.min(4.0 * thr + 1e-9));
            }
        }
        if ks.len() < 16 { return false; }
        let mut keep = vec![true; ks.len()];
        for _ in 0..3 {
            let (mut sw, mut sk, mut sf, mut skk, mut skf) = (0f64, 0f64, 0f64, 0f64, 0f64);
            for i in 0..ks.len() {
                if keep[i] { let w = ws[i]; sw += w; sk += w * ks[i]; sf += w * fs_[i]; skk += w * ks[i] * ks[i]; skf += w * ks[i] * fs_[i]; }
            }
            let den = sw * skk - sk * sk;
            if den <= 0.0 { return false; }
            let slope = (sw * skf - sk * sf) / den;
            let icpt = (sf - slope * sk) / sw;
            let res: Vec<f64> = ks.iter().enumerate().map(|(i, k)| (fs_[i] - (icpt + slope * k)).abs()).collect();
            let mut sr: Vec<f64> = res.iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, r)| *r).collect();
            sr.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let cut = if sr.is_empty() { 1.5 } else { (3.0 * sr[sr.len() / 2]).max(1.5) };
            keep = res.iter().map(|&r| r <= cut).collect();
            *p = slope; *a0 = icpt;
        }
        true
    };
    struct Pick { p: f64, phase: f64, contrast: f64, total: f64 }
    let mut pick: Option<Pick> = None;
    for &c in &cands {
        // bestNear(c, c*0.004, c*0.0005)
        let (span, step) = (c * 0.004, c * 0.0005);
        let mut best = fit(c);
        let mut bb = c;
        let mut cc = c - span;
        while cc <= c + span + 1e-9 { let f = fit(cc); if f.0 > best.0 { best = f; bb = cc; } cc += step; }
        let prior = ac_score(bb).max(0.0) / ac_max;
        let (rp, rphase) = (best.2, best.1);
        let mut p = rp;
        let mut a0 = rphase;
        if regress(&mut p, &mut a0, 0.12) { regress(&mut p, &mut a0, 0.06); }
        if (p / rp - 1.0).abs() > 0.01 { p = rp; a0 = rphase; }
        let ctr = contrast(p, ((a0 % p) + p) % p);
        let total = ctr * (0.6 + 0.4 * prior);
        if pick.as_ref().map_or(true, |pk| total > pk.total) { pick = Some(Pick { p, phase: a0, contrast: ctr, total }); }
    }
    let pick = pick.unwrap();
    let mut p = pick.p;
    let mut a0 = pick.phase;

    // ---- on the beat, or between the beats?
    let sum_at = |arr: &[f32], off: f64, a0: f64, p: f64| -> f64 {
        let mut s = 0f64;
        let mut k = (-(a0 + off) / p).ceil() as i64;
        while a0 + off + k as f64 * p < frames as f64 - 2.0 {
            if let Some(pk) = peak_near(arr, a0 + off + k as f64 * p, 2.0, frames) { s += pk.v; }
            k += 1;
        }
        s
    };
    let lm: Vec<f32> = (0..frames).map(|t| (o_l[t] + o_m[t] - 0.5 * o_h[t]).max(0.0)).collect();
    if sum_at(&lm, p / 2.0, a0, p) > 1.1 * sum_at(&lm, 0.0, a0, p) { a0 += p / 2.0; regress(&mut p, &mut a0, 0.06); }

    // ---- round to a whole (or half) BPM only when it can't cause audible drift
    let n_beats = frames as f64 / p;
    let mut bpm = 60.0 * fps / p;
    for q in [1.0, 2.0, 100.0] {
        let r = jsround(bpm * q) / q;
        let pr = 60.0 * fps / r;
        let drift_ms = (pr - p).abs() * n_beats / fps * 1000.0;
        if drift_ms < if q == 100.0 { 2.0 } else { 5.0 } { bpm = r; break; }
    }
    p = 60.0 * fps / bpm;
    {
        let (mut s, mut w) = (0f64, 0f64);
        let mut k = (-a0 / p).ceil() as i64;
        while a0 + k as f64 * p < frames as f64 - 2.0 {
            if let Some(pk) = peak_near(&beat_odf, a0 + k as f64 * p, p * 0.05, frames) {
                if pk.v >= thr { s += (pk.f - (a0 + k as f64 * p)) * pk.v; w += pk.v; }
            }
            k += 1;
        }
        if w != 0.0 { a0 += s / w; }
    }
    // a sharp attack peaks the flux about three quarters of the way into the window
    let beat_time = |k: i64| (a0 + k as f64 * p) / fps + 0.75 * N as f64 / fs;

    // ---- downbeat vote
    let k_first = (-a0 / p).ceil() as i64;
    let k_last = ((frames as f64 - 1.0 - a0) / p).floor() as i64 - 1;
    let nbt = k_last - k_first + 1;
    let mut vote = [0f64; 4];
    let res4 = |k: i64| (((k % 4) + 4) % 4) as usize;
    if nbt >= 16 {
        let nbt = nbt as usize;
        let mut f_kick = vec![0f64; nbt];
        let mut f_snare = vec![0f64; nbt];
        let mut f_entry = vec![0f64; nbt];
        let mut loud = vec![0f64; nbt];
        let mut avg_spec = vec![0f64; nbt * NB];
        for j in 0..nbt {
            let k = k_first + j as i64;
            let f0 = a0 + k as f64 * p;
            f_kick[j] = peak_near(&o_l, f0, 3.0, frames).map_or(0.0, |x| x.v);
            f_snare[j] = peak_near(&o_m, f0, 3.0, frames).map_or(0.0, |x| x.v);
            let s = jsround(f0).max(0.0) as usize;
            let e = (jsround(f0 + p) as usize).min(frames);
            let mut r = 0f64;
            let mut t = s;
            while t < e {
                r += rms[t] as f64;
                for q in 0..NB { avg_spec[j * NB + q] += spec[t * NB + q] as f64; }
                t += 1;
            }
            let c = (e.saturating_sub(s)).max(1) as f64;
            loud[j] = r / c;
            for q in 0..NB { avg_spec[j * NB + q] /= c; }
        }
        let mut f_change = vec![0f64; nbt];
        for j in 1..nbt {
            let mut d = 0f64;
            for q in 0..NB { d += (avg_spec[j * NB + q] - avg_spec[(j - 1) * NB + q]).abs(); }
            f_change[j] = d;
        }
        if nbt > 8 {
            for j in 4..nbt - 4 {
                let (mut a, mut b) = (0f64, 0f64);
                for q in 0..4 { a += loud[j + q]; b += loud[j - 4 + q]; }
                let r = ((a + 1e-6) / (b + 1e-6)).ln();
                f_entry[j] = if r > 0.4 { r } else { 0.0 };
            }
        }
        let zs = |v: &[f64]| -> Vec<f64> {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            let mut s = v.iter().map(|a| (a - m).powi(2)).sum::<f64>();
            s = (s / v.len() as f64).sqrt();
            if s == 0.0 { s = 1.0; }
            v.iter().map(|a| (a - m) / s).collect()
        };
        let z_k = zs(&f_kick);
        let z_s = zs(&f_snare);
        let _z_c = zs(&f_change);
        let mut nov = [0f64; 4];
        for r in 0..4i64 {
            let j0 = (((r - k_first) % 4) + 4) % 4;
            let mut prev_bar: Option<[f64; NB]> = None;
            let mut cnt = 0f64;
            let mut j = j0 as usize;
            while j + 4 <= nbt {
                let mut bar = [0f64; NB];
                for q in 0..4 { for u in 0..NB { bar[u] += avg_spec[(j + q) * NB + u] / 4.0; } }
                if let Some(pb) = prev_bar { let mut d = 0f64; for u in 0..NB { d += (bar[u] - pb[u]).abs(); } nov[r as usize] += d; cnt += 1.0; }
                prev_bar = Some(bar);
                j += 4;
            }
            nov[r as usize] /= if cnt > 0.0 { cnt } else { 1.0 };
        }
        let mut ks = [0f64; 4];
        let mut ksn = [0f64; 4];
        for j in 0..nbt { let r = res4(k_first + j as i64); ks[r] += z_k[j] - z_s[j]; ksn[r] += 1.0; }
        for r in 0..4 { ks[r] /= if ksn[r] > 0.0 { ksn[r] } else { 1.0 }; }
        let mut lsorted = loud.clone();
        lsorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let lmed = lsorted[nbt / 2];
        let mut j_start = 0usize;
        while j_start < nbt - 1 && loud[j_start] < 0.25 * lmed { j_start += 1; }
        let nov_mean = { let m = (nov[0] + nov[1] + nov[2] + nov[3]) / 4.0; if m == 0.0 { 1.0 } else { m } };
        for r in 0..4 { vote[r] = W_KS * ks[r] + W_NOV * (nov[r] / nov_mean - 1.0); }
        vote[res4(k_first + j_start as i64)] += W_START;
        for j in 0..nbt { vote[res4(k_first + j as i64)] += W_ENTRY * f_entry[j]; }
    }
    let mut best = 0usize;
    for r in 1..4 { if vote[r] > vote[best] { best = r; } }
    let mut k = best as i64;
    while beat_time(k - 4) >= 0.0 { k -= 4; }
    while beat_time(k) < 0.0 { k += 4; }
    Grid { bpm, first_beat: beat_time(k), conf: pick.contrast }
}

// ---- musical key -------------------------------------------------------------------------
// Temperley profiles, with the minor profile's 7ths swapped for natural (Aeolian) minor
const PROF_TPN: [[f64; 12]; 2] = [
    [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0],
    [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 4.0, 1.5],
];

#[derive(Clone, Copy, Debug)]
pub struct KeyResult { pub key: i32, pub conf: f64, pub tune_cents: f64 }

/// Spectral peaks from 8192-point FFTs -> tuning estimate -> 12-bin chroma (bass and upper range
/// kept separately) -> correlation against major/minor key profiles; best of 24 keys wins.
pub fn detect_key(x: &[f32], fs: f64) -> KeyResult {
    let (prom, pow, harm, hdecay, thr_rel, split, bass_w, fmax) = (3.0, 2.0, 5usize, 0.8f64, 0.02, 52.0, 0.5, 2100.0);
    const N: usize = 8192;
    let n = x.len();
    let hop = (N / 2).max(if n > N { (n - N) / 360 } else { 0 });
    let win = hann(N);
    let fft = Fft::new(N);
    let mut re = vec![0f64; N];
    let mut im = vec![0f64; N];
    let mut mag = vec![0f64; N / 2];
    let k_lo = (50.0 * N as f64 / fs).ceil() as usize;
    let k_hi = (N / 2 - 2).min((fmax * N as f64 / fs).floor() as usize);
    let (mut pm, mut pw) = (Vec::<f64>::new(), Vec::<f64>::new());
    let mut s = 0usize;
    while s + N <= n {
        for i in 0..N { re[i] = x[s + i] as f64 * win[i]; im[i] = 0.0; }
        fft.run(&mut re, &mut im);
        for k in 0..N / 2 { mag[k] = re[k].hypot(im[k]); }
        let mut mx = 0f64;
        for k in k_lo - 1..=k_hi + 1 { if mag[k] > mx { mx = mag[k]; } }
        s += hop;
        if mx < 1e-4 { continue; }
        let thr = mx * thr_rel;
        const R: i64 = 24;
        let m = |k: i64| -> f64 { if k >= 0 && (k as usize) < N / 2 { mag[k as usize] } else { 0.0 } };
        let mut run = 0f64;
        for k in k_lo as i64 - R..=k_lo as i64 + R { run += m(k); }
        for k in k_lo..=k_hi {
            if k > k_lo { let a = k as i64 + R; let b0 = k as i64 - R - 1; run += m(a) - m(b0); }
            let b = mag[k];
            if b < thr || b <= mag[k - 1] || b < mag[k + 1] { continue; }
            if b < prom * run / (2 * R + 1) as f64 { continue; }
            let la = (mag[k - 1] + 1e-12).ln();
            let lb = b.ln();
            let lc = (mag[k + 1] + 1e-12).ln();
            let den = la - 2.0 * lb + lc;
            let pp = k as f64 + if den < 0.0 { 0.5 * (la - lc) / den } else { 0.0 };
            let f = pp * fs / N as f64;
            pm.push(69.0 + 12.0 * (f / 440.0).log2());
            pw.push((b / mx).powf(pow));
        }
    }
    let (mut cs, mut sn) = (0f64, 0f64);
    for i in 0..pm.len() { let a = 2.0 * std::f64::consts::PI * pm[i]; cs += pw[i] * a.cos(); sn += pw[i] * a.sin(); }
    let tune = sn.atan2(cs) / (2.0 * std::f64::consts::PI);
    let mut bass = [0f64; 12];
    let mut high = [0f64; 12];
    for i in 0..pm.len() {
        let m = pm[i] - tune;
        let r = jsround(m);
        let dev = (m - r).abs();
        if dev > 0.4 { continue; }
        let w = pw[i] * (1.0 - dev / 0.4 * 0.5);
        for hh in 1..=harm {
            let mf = m - 12.0 * (hh as f64).log2();
            let rr = jsround(mf);
            if (mf - rr).abs() > 0.4 { continue; }
            let pc = (((rr as i64) % 12 + 12) % 12) as usize;
            let ww = w * hdecay.powi(hh as i32 - 1);
            if rr < split { bass[pc] += ww; } else { high[pc] += ww; }
        }
    }
    let norm = |v: &[f64; 12]| -> [f64; 12] { let s: f64 = v.iter().sum(); let s = if s == 0.0 { 1.0 } else { s }; let mut o = [0f64; 12]; for i in 0..12 { o[i] = v[i] / s; } o };
    let nb = norm(&bass);
    let nh = norm(&high);
    let mut chroma = [0f64; 12];
    for i in 0..12 { chroma[i] = nh[i] + bass_w * nb[i]; }
    let corr = |c: &[f64; 12], p: &[f64; 12], rot: usize| -> f64 {
        let mc = c.iter().sum::<f64>() / 12.0;
        let mp = p.iter().sum::<f64>() / 12.0;
        let (mut sxy, mut sxx, mut syy) = (0f64, 0f64, 0f64);
        for i in 0..12 { let a = c[(i + rot) % 12] - mc; let b = p[i] - mp; sxy += a * b; sxx += a * a; syy += b * b; }
        let d = (sxx * syy).sqrt();
        sxy / if d == 0.0 { 1.0 } else { d }
    };
    let mut scores = vec![];
    for m in 0..2 { for t in 0..12 { scores.push(corr(&chroma, &PROF_TPN[m], t)); } }
    let mut best = 0;
    for k in 1..24 { if scores[k] > scores[best] { best = k; } }
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    KeyResult { key: best as i32, conf: sorted[0] - sorted[1], tune_cents: tune * 100.0 }
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub wave: Option<Wave>,
    pub bpm: f64,
    pub first_beat: f64,
    pub grid_conf: f64,
    pub key: i32,
    pub key_conf: f64,
    pub phrases: Vec<Phrase>,
    pub duration: f64,
}

#[derive(Clone, Copy, Default)]
pub struct Opts {
    pub no_wave: bool,
    pub wave_only: bool,
    pub phrases_only: bool,
    /// keep a hand-edited grid: only the phrases (and key) are worked out around it
    pub grid: Option<(f64, f64)>,
}

/// x: ~11 kHz mono for the beat/key analysis; xw: optional ~22 kHz mono for the waveform
pub fn analyze(x: &[f32], fs: f64, xw: Option<(&[f32], f64)>, range: (f64, f64), opt: Opts) -> Analysis {
    let mut out = Analysis { key: -1, duration: x.len() as f64 / fs, ..Default::default() };
    if !opt.no_wave {
        let w = match xw { Some((w, f)) => waveform(w, f), None => waveform(x, fs) };
        out.wave = Some(w);
    }
    if opt.wave_only { return out; }
    let g = match opt.grid { Some((bpm, fb)) => Grid { bpm, first_beat: fb, conf: 0.0 }, None => beat_grid(x, fs, range) };
    out.bpm = g.bpm; out.first_beat = g.first_beat; out.grid_conf = g.conf;
    out.phrases = phrase_analysis(x, fs, g.bpm, g.first_beat);
    if opt.phrases_only { return out; }
    let k = detect_key(x, fs);
    out.key = k.key; out.key_conf = k.conf;
    out
}

/// BPM ranges, like Serato's and Traktor's: where half/double-time tempos get folded to. 70-180 is
/// wider than an octave, so most tracks keep the tempo the analyser hears. After these: Custom.
pub const BPM_RANGES: [(&str, f64, f64); 5] = [("58-115", 58.0, 116.0), ("68-135", 68.0, 136.0), ("78-155", 78.0, 156.0), ("88-175", 88.0, 176.0), ("70-180", 70.0, 181.0)];
/// the bpm_range index of the custom range (settings bpm_lo..bpm_hi)
pub const BPM_CUSTOM: usize = BPM_RANGES.len();
/// a custom range made usable: whole numbers, and at least an octave wide so every tempo folds into it
pub fn custom_range(lo: f64, hi: f64) -> (f64, f64) {
    let lo = lo.round().clamp(40.0, 150.0);
    (lo, hi.round().clamp(2.0 * lo - 1.0, 300.0) + 1.0)
}
pub fn fold_bpm(b: f64, range: (f64, f64)) -> f64 {
    if !(b > 0.0) { return b; }
    let mut b = b;
    while b < range.0 { b *= 2.0; }
    while b >= range.1 { b /= 2.0; }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bpm_ranges() {
        let wide = (BPM_RANGES[4].1, BPM_RANGES[4].2);
        assert_eq!(fold_bpm(174.0, wide), 174.0);
        assert_eq!(fold_bpm(70.0, wide), 70.0);
        assert_eq!(fold_bpm(180.0, wide), 180.0);
        assert_eq!(fold_bpm(181.0, wide), 90.5);
        assert_eq!(fold_bpm(65.0, wide), 130.0);
        // custom: kept at least an octave wide, top end inclusive
        assert_eq!(custom_range(85.0, 100.0), (85.0, 170.0));
        assert_eq!(custom_range(60.0, 200.0), (60.0, 201.0));
        let r = custom_range(85.0, 100.0);
        for b in [40.0, 84.0, 169.0, 170.0, 333.0] { let f = fold_bpm(b, r); assert!(f >= r.0 && f < r.1, "{b} -> {f}"); }
    }
}
