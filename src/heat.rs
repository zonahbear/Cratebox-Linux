//! HEAT: TrueCurve heat curves for a Puffco Peak Pro / Proxy, and HOLD for the PUFF Beat FX. A port of the iOS
//! personal build's TrueCurve.swift + PuffcoBle.swift + HeatController.swift (themselves from cratebox.html's
//! TC section and zonahbear/truecurve): same curve maths (monotone cubic or straight lines), predictive driver,
//! heater-model learning, Lorax byte formats and safety rules (curves on profile slot 4, backed up and put back;
//! 600°F cutoff; targets clamped 350-590°F; on a lost link the device finishes its own cycle).
//! Threads: `Heat` is the UI side; a worker thread owns the device (BlueZ over D-Bus via zbus on Linux,
//! WinRT Bluetooth LE on Windows (heat_win.rs), or the simulator) and does every blocking read and write. It never touches the audio engine: PUFF amounts reach
//! the BEAT FX unit through the app as ordinary commands. Temperatures are °F, times seconds.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// targets are clamped to this range; a reading at or above `CUTOFF_F` stops the run
pub const MIN_F: f64 = 350.0;
pub const MAX_F: f64 = 590.0;
pub const CUTOFF_F: f64 = 600.0;
/// curves run on profile slot 4 (index 3), backed up first and restored after
pub const SLOT: u8 = 3;
/// Proxy reports "ready" (10) for most of a heat cycle, not just "heating" (8)
fn heating(s: Option<i32>) -> bool { matches!(s, Some(7 | 8 | 10)) }
pub fn c_to_f(c: f64) -> f64 { c * 9.0 / 5.0 + 32.0 }
pub fn f_to_c(f: f64) -> f64 { (f - 32.0) * 5.0 / 9.0 }
fn clamp(v: f64, lo: f64, hi: f64) -> f64 { v.max(lo).min(hi) }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point { pub t: f64, pub f: f64 }
/// the clipboard / saved form: {name: {points: [{t, f}], smooth}}, same as TrueCurve's tc_curves
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurveDef { pub points: Vec<Point>, #[serde(default = "yes")] pub smooth: bool }
fn yes() -> bool { true }

/// TrueCurve's presets, in menu order; Quartz Banger (starts at 520 and trails off to 450) is the default
pub const DEFAULT_NAME: &str = "Quartz Banger";
pub const PRESETS: [(&str, &[(f64, f64)]); 6] = [
    ("Quartz Banger", &[(0.0, 520.0), (10.0, 508.0), (25.0, 485.0), (45.0, 462.0), (65.0, 450.0)]),
    ("Terp Ramp", &[(0.0, 450.0), (20.0, 480.0), (40.0, 520.0), (55.0, 550.0)]),
    ("Hot Start, Cool Finish", &[(0.0, 540.0), (15.0, 520.0), (35.0, 480.0), (55.0, 460.0)]),
    ("Wave", &[(0.0, 470.0), (15.0, 520.0), (30.0, 480.0), (45.0, 540.0), (60.0, 505.0)]),
    ("Slow Climb", &[(0.0, 430.0), (50.0, 510.0), (80.0, 560.0)]),
    ("Flat 500", &[(0.0, 500.0), (45.0, 500.0)]),
];
pub fn preset(name: &str) -> Option<CurveDef> {
    PRESETS.iter().find(|p| p.0 == name).map(|p| CurveDef { points: p.1.iter().map(|&(t, f)| Point { t, f }).collect(), smooth: true })
}

// ===================================================================== curve maths

/// monotone cubic (Fritsch–Butland: never overshoots between points) or straight lines
pub struct Curve { xs: Vec<f64>, ys: Vec<f64>, hh: Vec<f64>, d: Vec<f64>, m: Vec<f64>, pub smooth: bool, pub end: f64 }
impl Curve {
    /// None with fewer than two distinct times
    pub fn new(points: &[Point], smooth: bool) -> Option<Curve> {
        let mut src: Vec<Point> = points.iter().filter(|p| p.t.is_finite() && p.f.is_finite()).copied().collect();
        src.sort_by(|a, b| a.t.total_cmp(&b.t));
        let mut pts: Vec<Point> = vec![];
        for p in src { if pts.last().map(|l| l.t) != Some(p.t) { pts.push(p); } }
        if pts.len() < 2 { return None; }
        let n = pts.len();
        let xs: Vec<f64> = pts.iter().map(|p| p.t).collect();
        let ys: Vec<f64> = pts.iter().map(|p| p.f).collect();
        let (mut hh, mut d, mut m) = (vec![0.0; n - 1], vec![0.0; n - 1], vec![0.0; n]);
        for i in 0..n - 1 { hh[i] = xs[i + 1] - xs[i]; d[i] = (ys[i + 1] - ys[i]) / hh[i]; }
        for i in 1..n - 1 {
            if d[i - 1] * d[i] > 0.0 {
                let (w1, w2) = (2.0 * hh[i] + hh[i - 1], hh[i] + 2.0 * hh[i - 1]);
                m[i] = (w1 + w2) / (w1 / d[i - 1] + w2 / d[i]);
            }
        }
        let end = xs[n - 1];
        Some(Curve { xs, ys, hh, d, m, smooth, end })
    }
    fn seg(&self, t: f64) -> usize { let mut i = 0; while i < self.xs.len() - 2 && t > self.xs[i + 1] { i += 1; } i }
    pub fn at(&self, t: f64) -> f64 {
        let t = clamp(t, 0.0, self.end);
        let i = self.seg(t);
        let s = (t - self.xs[i]) / self.hh[i];
        let (ys, hh, m) = (&self.ys, &self.hh, &self.m);
        if !self.smooth { return ys[i] + s * (ys[i + 1] - ys[i]); }
        let (s2, s3) = (s * s, s * s * s);
        (2.0 * s3 - 3.0 * s2 + 1.0) * ys[i] + (s3 - 2.0 * s2 + s) * hh[i] * m[i] + (-2.0 * s3 + 3.0 * s2) * ys[i + 1] + (s3 - s2) * hh[i] * m[i + 1]
    }
    pub fn slope(&self, t: f64) -> f64 {
        if t <= 0.0 || t >= self.end { return 0.0; }
        let i = self.seg(t);
        let s = (t - self.xs[i]) / self.hh[i];
        if !self.smooth { return self.d[i]; }
        let (ys, hh, m) = (&self.ys, &self.hh, &self.m);
        let s2 = s * s;
        ((6.0 * s2 - 6.0 * s) * ys[i] + (-6.0 * s2 + 6.0 * s) * ys[i + 1]) / hh[i] + (3.0 * s2 - 4.0 * s + 1.0) * m[i] + (3.0 * s2 - 2.0 * s) * m[i + 1]
    }
}

// ===================================================================== heater model, driver, learner

/// bowl temp follows the heater target as a first-order lag capped by heating power; it can only cool passively
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeaterModel { pub tau: f64, #[serde(rename = "heatMax")] pub heat_max: f64, #[serde(rename = "coolK")] pub cool_k: f64, pub dead: f64, pub runs: u32 }
impl Default for HeaterModel { fn default() -> Self { HeaterModel { tau: 4.0, heat_max: 12.0, cool_k: 0.015, dead: 0.6, runs: 0 } } }

pub fn plant_step(y: f64, u: f64, dt: f64, p: &HeaterModel, extra_cool: f64) -> f64 {
    let rate = if u > y { ((u - y) / p.tau).min(p.heat_max) } else { ((u - y) / p.tau).max(-p.cool_k * (y - 70.0)) };
    y + (rate - extra_cool) * dt
}

/// look ahead by dead time + BLE latency and invert the lag; live correction only trims (big asks can make
/// the firmware abort the session)
pub struct Driver<'a> { c: &'a Curve, m: HeaterModel, i: f64 }
impl<'a> Driver<'a> {
    pub fn new(c: &'a Curve, m: HeaterModel) -> Driver<'a> { Driver { c, m, i: 0.0 } }
    pub fn command(&mut self, t: f64, y: Option<f64>, dt: f64) -> f64 {
        let (c, m) = (self.c, &self.m);
        let tl = t + m.dead + 0.25;
        let r = c.at(t);
        let ff = c.at(tl) + m.tau * if tl < c.end { c.slope(tl) } else { 0.0 };
        let (mut corr, mut i, mut unsat) = (0.0, self.i, true);
        if let Some(y) = y {
            let e = r - y;
            if e.abs() < 30.0 { i = clamp(i + 0.1 * e * dt, -12.0, 12.0); }
            let raw = 0.6 * clamp(e, -30.0, 30.0) + i;
            corr = clamp(raw, -15.0, 20.0);
            unsat = corr == raw;
        }
        let u = ff + corr;
        let uc = clamp(u, MIN_F.max(r - 40.0), MAX_F.min(r + 30.0));
        if unsat && (uc - u).abs() < 0.01 { self.i = i; }
        uc
    }
}

/// learns tau / max heat rate / cooling from a run, like TrueCurve (stored per device)
pub struct Learner { pub m: HeaterModel, pub n: u32, s: std::collections::VecDeque<(f64, f64, f64)> }
impl Learner {
    pub fn new(m: HeaterModel) -> Learner { Learner { m, n: 0, s: Default::default() } }
    pub fn add(&mut self, t: f64, y: Option<f64>, u: f64) {
        let Some(y) = y else { return };
        self.s.push_back((t, y, u));
        while self.s.front().is_some_and(|f| t - f.0 > 1.4) { self.s.pop_front(); }
        if self.s.len() < 4 { return; }
        let k = self.s.len() as f64;
        let mt = self.s.iter().map(|v| v.0).sum::<f64>() / k;
        let my = self.s.iter().map(|v| v.1).sum::<f64>() / k;
        let (mut num, mut den) = (0.0, 0.0);
        for v in &self.s { num += (v.0 - mt) * (v.1 - my); den += (v.0 - mt) * (v.0 - mt); }
        if den < 0.2 { return; }
        let m = &mut self.m;
        let dy = num / den;
        let ta = t - m.dead - 0.2;
        let ud = self.s.iter().filter(|v| v.0 <= ta).last().map(|v| v.2).unwrap_or(self.s[0].2);
        let gap = ud - my;
        if gap > 10.0 && dy > 1.0 {
            if gap / m.tau < 0.7 * m.heat_max { m.tau += 0.04 * (clamp(gap / dy, 1.5, 15.0) - m.tau); }
            else { m.heat_max += if dy > m.heat_max { 0.15 } else { 0.03 } * (clamp(dy, 3.0, 40.0) - m.heat_max); }
            self.n += 1;
        } else if gap < -10.0 && dy < -0.5 && my > 150.0 && -gap / m.tau > 1.3 * m.cool_k * (my - 70.0) {
            m.cool_k += 0.05 * (clamp(-dy / (my - 70.0), 0.003, 0.06) - m.cool_k);
            self.n += 1;
        }
    }
}

/// PUFF (Beat FX): turns a HOLD's bowl temperature into an effect amount. The device holds one temperature;
/// a draw cools the bowl and the heater fights back, so the dip below the settled temperature says how hard
/// you're hitting it. The settled temperature jumps up to any higher reading, stays put while the bowl is
/// falling (a pull), follows a recovery down in ~1.5 s and drifts to a steady reading in ~10 s. Nothing reads
/// until the bowl first reaches the hold temperature. The dip also counts how fast it's falling, and with
/// `full` = 0 (Auto) the full warble scales to your biggest dip lately (85% of it, 6°F at least).
pub struct PuffDrop { pub dead: f64, pub full: f64, pub settled: Option<f64>, pub drop: f64, pub amount: f64, pub rate: f64, pub peak: f64, hold: f64, last: Option<f64>, last_y: f64 }
impl PuffDrop {
    pub fn new(hold: f64, full: f64) -> PuffDrop {
        let dead = 1.5;
        PuffDrop { dead, full: if full > 0.0 { full.max(dead + 1.0) } else { 0.0 }, settled: None, drop: 0.0, amount: 0.0, rate: 0.0, peak: 0.0, hold, last: None, last_y: 0.0 }
    }
    /// the dip that makes the full warble right now
    pub fn scale(&self) -> f64 { if self.full > 0.0 { self.full } else { (self.peak * 0.85).clamp(6.0, 80.0) } }
    /// one reading (°F, None = none) at time t (s); returns the amount, 0..1
    pub fn add(&mut self, t: f64, y: Option<f64>) -> f64 {
        let Some(y) = y else { return self.amount };
        let dt = self.last.map(|l| clamp(t - l, 0.0, 1.0)).unwrap_or(0.0);
        self.last = Some(t);
        let Some(mut b) = self.settled else {
            if y >= self.hold - 12.0 { self.settled = Some(y); self.last_y = y; }
            return 0.0;
        };
        if dt > 0.0 { self.rate += ((y - self.last_y) / dt - self.rate) * (1.0 - (-dt / 0.4).exp()); }
        self.last_y = y;
        // never above the hold temperature + a little: an overshoot isn't the new normal
        if y > b { b = y.min(self.hold + 10.0); }
        else if self.rate >= -1.0 { b += (y - b) * (1.0 - (-dt / if self.rate > 1.0 { 1.5 } else { 10.0 }).exp()); }
        self.settled = Some(b);
        self.drop = (b - y).max(0.0) + (-self.rate - 2.0).max(0.0) * 0.5;
        self.peak = (self.peak * (-dt / 60.0).exp()).max(self.drop);
        self.amount = clamp((self.drop - self.dead) / (self.scale() - self.dead), 0.0, 1.0);
        self.amount
    }
}

/// the simulator's heater: a "truth" model that differs from the driver's default, a dead time on live
/// targets, and a device that ends its own cycle after the slot's time
pub struct SimHeater { pub truth: HeaterModel, pub y: f64, pub u: f64, pub state: i32, pub slot_idx: usize, pub airflow: f64, pub slots: [(f64, f64); 4], pub clock: f64, cmds: Vec<(f64, f64)>, end_at: f64 }
impl Default for SimHeater {
    fn default() -> Self {
        SimHeater { truth: HeaterModel { tau: 3.0, heat_max: 15.0, cool_k: 0.011, dead: 0.45, runs: 0 }, y: 74.0, u: 74.0, state: 5, slot_idx: 0, airflow: 0.0,
            slots: [(485.0, 45.0); 4], clock: 0.0, cmds: vec![], end_at: 0.0 }
    }
}
impl SimHeater {
    pub fn advance(&mut self, dt: f64) {
        self.clock += dt;
        while self.cmds.first().is_some_and(|c| c.0 <= self.clock) { self.u = self.cmds.remove(0).1; }
        let on = self.state == 7 || self.state == 8;
        self.y = plant_step(self.y, if on { self.u } else { 70.0 }, dt, &self.truth, self.airflow * 12.0);
        if self.state == 7 && self.y >= self.u - 6.0 { self.state = 8; self.end_at = self.clock + self.slots[self.slot_idx].1; }
        if self.state == 8 && self.clock >= self.end_at { self.state = 5; }
    }
    pub fn start(&mut self) { self.state = 7; self.u = self.slots[self.slot_idx].0; self.cmds.clear(); }
    pub fn stop(&mut self) { self.state = 5; }
    pub fn set_live_target(&mut self, f: f64) { self.cmds.push((self.clock + self.truth.dead, clamp(f, MIN_F, MAX_F))); }
}

/// TrueCurve's "Copy curves for Cratebox" JSON ({name: {points: [{t, f}], smooth}}); None if it isn't that.
/// Points are clamped like the web app's paste (t 0-110 s, f 350-590°F), names cut to 24 characters.
pub fn parse_curves(text: &str) -> Option<HashMap<String, CurveDef>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let obj = v.as_object()?;
    let num = |v: Option<&serde_json::Value>| -> Option<f64> { match v? { serde_json::Value::Number(n) => n.as_f64(), serde_json::Value::String(s) => s.trim().parse().ok(), _ => None } };
    let mut out = HashMap::new();
    for (k, c) in obj {
        let Some(raw) = c.get("points").and_then(|p| p.as_array()) else { continue };
        let pts: Vec<Point> = raw.iter().filter_map(|p| {
            let (t, f) = (num(p.get("t"))?, num(p.get("f"))?);
            (t.is_finite() && f.is_finite()).then(|| Point { t: clamp(t, 0.0, 110.0), f: clamp(f, MIN_F, MAX_F) })
        }).collect();
        if pts.len() >= 2 { out.insert(k.chars().take(24).collect(), CurveDef { points: pts, smooth: c.get("smooth").and_then(|s| s.as_bool()) != Some(false) }); }
    }
    Some(out)
}

// ===================================================================== the Lorax protocol's bytes

/// frames are [seq lo, seq hi, op, payload], replies [seq lo, seq hi, status, data]. Ops: 0 seed, 1 unlock,
/// 16 read, 17 write. Temperatures on the wire are °C, as float32 (Peak Pro) or int32 tenths (Proxy, "i10").
pub mod lorax {
    use sha2::{Digest, Sha256};
    pub const SERVICE: &str = "e276967f-ea8a-478a-a92e-d78f5dd15dd5";
    pub const SERVICE_LEGACY: &str = "06caf9c0-74d3-454f-9be9-e30cd999c17a";
    pub const CMD: &str = "60133d5c-5727-4f2c-9697-d842c5292a3c";
    pub const REPLY: &str = "8dc5ec05-8f7d-45ad-99db-3fbde65dbd9c";
    pub const VERSION: &str = "05434bca-cc7f-4ef6-bbb3-b1c520b9800c";
    const KEY: [u8; 16] = [0x64, 0xc6, 0x45, 0x62, 0x56, 0xf2, 0x6f, 0x5b, 0x1c, 0xa1, 0x27, 0x37, 0xa5, 0xdd, 0x71, 0xfb];
    pub const MODE: &str = "/p/app/mc";
    pub const STATE: &str = "/p/app/stat/id";
    pub const TEMP: &str = "/p/app/htr/temp";
    pub const TEMP_ALT: &str = "/p/htr/temp";
    pub const LIVE: &str = "/p/app/thc/temp";
    pub const ACTIVE_SLOT: &str = "/p/app/hcs";
    pub const SOC: &str = "/p/bat/soc";
    pub fn p_temp(s: u8) -> String { format!("/u/app/hc/{s}/temp") }
    pub fn p_time(s: u8) -> String { format!("/u/app/hc/{s}/time") }
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Fmt { F32, I10 }

    pub fn frame(seq: u16, op: u8, payload: &[u8]) -> Vec<u8> { let mut v = vec![(seq & 0xff) as u8, (seq >> 8) as u8, op]; v.extend_from_slice(payload); v }
    pub fn read_payload(path: &str, max: u16) -> Vec<u8> { let mut v = vec![0, 0, (max & 0xff) as u8, (max >> 8) as u8]; v.extend_from_slice(path.as_bytes()); v }
    pub fn write_payload(path: &str, bytes: &[u8]) -> Vec<u8> { let mut v = vec![0, 0, 0]; v.extend_from_slice(path.as_bytes()); v.push(0); v.extend_from_slice(bytes); v }
    /// unlock token: the first 16 bytes of SHA-256(key + the 16-byte seed)
    pub fn token(seed: &[u8]) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update(KEY);
        h.update(&seed[..seed.len().min(16)]);
        h.finalize()[..16].to_vec()
    }
    fn u32le(b: &[u8]) -> u32 { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) }
    pub fn f32v(b: &[u8]) -> f64 { f32::from_bits(u32le(b)) as f64 }
    pub fn i32v(b: &[u8]) -> i64 { u32le(b) as i32 as i64 }
    pub fn f32_bytes(v: f64) -> Vec<u8> { (v as f32).to_bits().to_le_bytes().to_vec() }
    /// a °C reading: float unless the device is known to be i10 or the float is implausible
    pub fn decode_c(b: Option<&[u8]>, fmt: Option<Fmt>) -> Option<f64> {
        let b = b.filter(|b| b.len() >= 4)?;
        let f = f32v(b);
        if fmt != Some(Fmt::I10) && f.is_finite() && f > -80.0 && f < 520.0 && !(f != 0.0 && f.abs() < 1e-3) { return Some(f); }
        let i = i32v(b);
        (i.abs() <= 6000).then(|| i as f64 / 10.0)
    }
    pub fn encode_c(c: f64, fmt: Fmt) -> Vec<u8> { if fmt == Fmt::I10 { (((c * 10.0).round()) as i32).to_le_bytes().to_vec() } else { f32_bytes(c) } }
    /// what a profile temperature reads back as
    pub fn decode_as(b: &[u8], fmt: Fmt) -> f64 { if fmt == Fmt::I10 { i32v(b) as f64 / 10.0 } else { f32v(b) } }
    /// slot 0's profile temperature tells the format (a profile is 100-350 °C)
    pub fn detect_format(b: Option<&[u8]>, is_proxy: bool) -> Fmt {
        if let Some(b) = b.filter(|b| b.len() >= 4) {
            let (f, i) = (f32v(b), i32v(b));
            if f.is_finite() && (100.0..=350.0).contains(&f) { return Fmt::F32; }
            if (1000..=3600).contains(&i) { return Fmt::I10; }
        }
        if is_proxy { Fmt::I10 } else { Fmt::F32 }
    }
    /// the live target (/p/app/thc/temp) can be in a different format from the profiles
    pub fn detect_live_format(b: Option<&[u8]>) -> Option<Fmt> {
        let b = b.filter(|b| b.len() >= 4)?;
        let (f, i) = (f32v(b), i32v(b));
        if f.is_finite() && (150.0..=400.0).contains(&f) { return Some(Fmt::F32); }
        if (1500..=4000).contains(&i) { return Some(Fmt::I10); }
        None
    }
    /// /p/bat/soc: float on Peak Pro, integer (sometimes tenths/hundredths of a percent) on Proxy
    #[derive(Default)]
    pub struct SocDecoder { scale: i64 }
    impl SocDecoder {
        pub fn decode(&mut self, b: Option<&[u8]>) -> Option<i64> {
            let b = b.filter(|b| !b.is_empty())?;
            if b.len() >= 4 {
                let f = f32v(b);
                if f.is_finite() && (1e-3..1.0).contains(&f) { return Some((f * 100.0).round() as i64); }
                if f.is_finite() && (1.0..=100.0).contains(&f) { return Some(f.round() as i64); }
            }
            let i = if b.len() >= 4 { i32v(b) } else if b.len() >= 2 { u16::from_le_bytes([b[0], b[1]]) as i64 } else { b[0] as i64 };
            if !(0..=10000).contains(&i) { return None; }
            if i > 1000 { self.scale = 100; } else if i > 100 && self.scale != 100 { self.scale = 10; }
            Some((i as f64 / self.scale.max(1) as f64).round() as i64)
        }
    }
}
use lorax::Fmt;

// ===================================================================== devices

#[derive(Clone, Default)]
pub struct SlotBackup { temp: Option<Vec<u8>>, time: Option<Vec<u8>> }

/// what the runner needs from a device (the Puffco or the simulator); every call blocks the worker thread only
trait Device: Send {
    fn name(&self) -> String;
    fn id(&self) -> String;
    fn connected(&mut self) -> bool;
    fn disconnect(&mut self);
    fn read_temp(&mut self) -> Option<f64>;
    fn read_state(&mut self) -> Option<i32>;
    fn read_battery(&mut self) -> Option<i64>;
    fn read_active_slot(&mut self) -> Option<u8>;
    fn set_active_slot(&mut self, s: u8) -> bool;
    fn backup_slot(&mut self, s: u8) -> SlotBackup;
    fn restore_slot(&mut self, s: u8, b: &SlotBackup);
    fn write_profile_temp(&mut self, s: u8, f: f64) -> bool;
    fn write_profile_time(&mut self, s: u8, secs: f64) -> bool;
    fn start(&mut self) -> bool;
    fn stop(&mut self) -> bool;
    fn probe_live_format(&mut self);
    fn set_live_target(&mut self, f: f64) -> bool;
    fn is_sim(&self) -> bool { false }
    /// the simulator's stand-in for a draw (0..1)
    fn set_airflow(&mut self, _a: f64) {}
}

/// the web app's simulator: SimHeater ticking in real time, with a little I/O delay and sensor noise
struct SimDevice { h: SimHeater, last: Instant, on: bool, rng: u64 }
impl SimDevice {
    fn new() -> SimDevice { SimDevice { h: SimHeater::default(), last: Instant::now(), on: true, rng: 0x9e37_79b9_7f4a_7c15 ^ Instant::now().elapsed().as_nanos() as u64 } }
    fn rand(&mut self) -> f64 { self.rng ^= self.rng << 13; self.rng ^= self.rng >> 7; self.rng ^= self.rng << 17; (self.rng >> 11) as f64 / (1u64 << 53) as f64 }
    fn tick(&mut self) {
        let mut dt = self.last.elapsed().as_secs_f64().min(5.0);
        self.last = Instant::now();
        while dt > 0.0 { let s = dt.min(0.1); self.h.advance(s); dt -= s; }
    }
    fn io(&mut self) { let ms = 30 + (self.rand() * 30.0) as u64; std::thread::sleep(Duration::from_millis(ms)); self.tick(); }
}
impl Device for SimDevice {
    fn name(&self) -> String { "Simulator".into() }
    fn id(&self) -> String { "sim".into() }
    fn connected(&mut self) -> bool { self.on }
    fn disconnect(&mut self) { self.on = false; }
    fn read_temp(&mut self) -> Option<f64> { self.io(); let n = (self.rand() - 0.5) * 1.2; Some(((self.h.y + n) * 10.0).round() / 10.0) }
    fn read_state(&mut self) -> Option<i32> { self.io(); Some(self.h.state) }
    fn read_battery(&mut self) -> Option<i64> { Some(87) }
    fn read_active_slot(&mut self) -> Option<u8> { Some(self.h.slot_idx as u8) }
    fn set_active_slot(&mut self, s: u8) -> bool { self.h.slot_idx = s as usize; true }
    fn backup_slot(&mut self, s: u8) -> SlotBackup { let (t, d) = self.h.slots[s as usize]; SlotBackup { temp: Some(lorax::f32_bytes(t)), time: Some(lorax::f32_bytes(d)) } }
    fn restore_slot(&mut self, s: u8, b: &SlotBackup) {
        if let Some(t) = b.temp.as_ref().filter(|t| t.len() >= 4) { self.h.slots[s as usize].0 = lorax::f32v(t); }
        if let Some(t) = b.time.as_ref().filter(|t| t.len() >= 4) { self.h.slots[s as usize].1 = lorax::f32v(t); }
    }
    fn write_profile_temp(&mut self, s: u8, f: f64) -> bool { self.io(); self.h.slots[s as usize].0 = f; true }
    fn write_profile_time(&mut self, s: u8, secs: f64) -> bool { self.io(); self.h.slots[s as usize].1 = secs; true }
    fn start(&mut self) -> bool { self.io(); self.h.start(); true }
    fn stop(&mut self) -> bool { self.io(); self.h.stop(); true }
    fn probe_live_format(&mut self) {}
    fn set_live_target(&mut self, f: f64) -> bool { self.io(); self.h.set_live_target(f); true }
    fn is_sim(&self) -> bool { true }
    fn set_airflow(&mut self, a: f64) { self.tick(); self.h.airflow = clamp(a, 0.0, 1.0); }
}

/// The Lorax side of a Puffco link, the same over BlueZ and WinRT: the unlock handshake, reads and writes
/// by path, and the `Device` calls. The transport's `Ble` brings `send` (one framed request and its
/// reply), `up` and `close`, and the fields used here.
macro_rules! lorax_device {
    ($t:ty) => {
        impl $t {
            fn auth(&mut self) -> String {
                match self.send(0, &[], 3.0) {
                    Ok((st, seed)) if st == 0 && seed.len() >= 16 => match self.send(1, &lorax::token(&seed), 3.0) {
                        Ok((0, _)) => "ok".into(),
                        Ok((st, _)) => format!("unlock returned status {st}"),
                        Err(e) => if e.contains("timed out") { "no reply from device".into() } else { e },
                    },
                    Ok((st, _)) => format!("seed request returned status {st}"),
                    Err(e) => if e.contains("timed out") { "no reply from device".into() } else { e },
                }
            }
            fn read(&mut self, path: &str, max: u16) -> Option<Vec<u8>> { match self.send(16, &lorax::read_payload(path, max), 3.0) { Ok((0, d)) => Some(d), _ => None } }
            fn write(&mut self, path: &str, bytes: &[u8]) -> bool { matches!(self.send(17, &lorax::write_payload(path, bytes), 3.0), Ok((0, _))) }
        }

        impl Device for $t {
            fn name(&self) -> String { self.name.clone() }
            fn id(&self) -> String { self.addr.clone() }
            fn connected(&mut self) -> bool { self.up() }
            fn disconnect(&mut self) { self.close() }
            fn read_temp(&mut self) -> Option<f64> {
                let order = if self.temp_path == lorax::TEMP_ALT { [lorax::TEMP_ALT, lorax::TEMP] } else { [lorax::TEMP, lorax::TEMP_ALT] };
                for path in order {
                    let b = self.read(path, 4);
                    if let Some(c) = lorax::decode_c(b.as_deref(), Some(self.fmt)).filter(|c| *c > 0.0) { self.temp_path = path; return Some((c_to_f(c) * 10.0).round() / 10.0); }
                }
                None
            }
            fn read_state(&mut self) -> Option<i32> { self.read(lorax::STATE, 1).and_then(|b| b.first().map(|&x| x as i32)) }
            fn read_battery(&mut self) -> Option<i64> { let b = self.read(lorax::SOC, 4); self.soc.decode(b.as_deref()) }
            fn read_active_slot(&mut self) -> Option<u8> { self.read(lorax::ACTIVE_SLOT, 1).and_then(|b| b.first().copied()) }
            fn set_active_slot(&mut self, s: u8) -> bool { self.write(lorax::ACTIVE_SLOT, &[s]) }
            fn backup_slot(&mut self, s: u8) -> SlotBackup { SlotBackup { temp: self.read(&lorax::p_temp(s), 4), time: self.read(&lorax::p_time(s), 4) } }
            fn restore_slot(&mut self, s: u8, b: &SlotBackup) {
                if let Some(t) = b.temp.as_ref().filter(|t| t.len() >= 4) { self.write(&lorax::p_temp(s), &t[..4]); }
                if let Some(t) = b.time.as_ref().filter(|t| t.len() >= 4) { self.write(&lorax::p_time(s), &t[..4]); }
            }
            fn write_profile_temp(&mut self, s: u8, f: f64) -> bool {
                let c = f_to_c(clamp(f, MIN_F, MAX_F));
                let other = if self.fmt == Fmt::I10 { Fmt::F32 } else { Fmt::I10 };
                for fm in [self.fmt, other] {
                    if !self.write(&lorax::p_temp(s), &lorax::encode_c(c, fm)) { continue; }
                    if let Some(b) = self.read(&lorax::p_temp(s), 4) { if b.len() >= 4 && (lorax::decode_as(&b, fm) - c).abs() > 1.5 { continue; } }
                    self.fmt = fm;
                    return true;
                }
                false
            }
            /// Peak Pro: heat time in float seconds. Proxy units vary by firmware, so it's left alone and the run ends the session.
            fn write_profile_time(&mut self, s: u8, secs: f64) -> bool {
                if self.is_proxy { return false; }
                if !self.write(&lorax::p_time(s), &lorax::f32_bytes(secs)) { return false; }
                match self.read(&lorax::p_time(s), 4) { Some(r) if r.len() >= 4 => (lorax::f32v(&r) - secs).abs() <= 1.0, _ => true }
            }
            fn start(&mut self) -> bool { self.write(lorax::MODE, &[7]) }
            fn stop(&mut self) -> bool {
                let mut ok = false;
                for _ in 0..3 { if self.write(lorax::MODE, &[8]) { ok = true; } std::thread::sleep(Duration::from_millis(45)); }
                ok
            }
            fn probe_live_format(&mut self) { let b = self.read(lorax::LIVE, 4); if let Some(f) = lorax::detect_live_format(b.as_deref()) { self.live_fmt = Some(f); } }
            fn set_live_target(&mut self, f: f64) -> bool {
                let c = f_to_c(clamp(f, MIN_F, MAX_F));
                let first = self.live_fmt.unwrap_or(self.fmt);
                for fm in [first, if first == Fmt::I10 { Fmt::F32 } else { Fmt::I10 }] {
                    if self.write(lorax::LIVE, &lorax::encode_c(c, fm)) { self.live_fmt = Some(fm); return true; }
                }
                false
            }
        }
    };
}

// the Bluetooth side: the same Bus / Seen / Ble on each system
#[cfg(target_os = "linux")]
use bluez as ble;
#[cfg(windows)]
use winble as ble;
#[cfg(windows)]
#[path = "heat_win.rs"]
mod winble;

/// BlueZ over the system D-Bus (zbus, pure Rust). Lubuntu's stock Bluetooth stack runs bluetoothd.
#[cfg(target_os = "linux")]
mod bluez {
    use super::*;
    use zbus::blocking::{Connection, MessageIterator, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
    const DEV: &str = "org.bluez.Device1";
    const CHR: &str = "org.bluez.GattCharacteristic1";

    pub struct Bus { pub conn: Connection, adapter: OwnedObjectPath }
    #[derive(Clone)]
    pub struct Seen { pub path: String, pub addr: String, pub name: String, pub puffco: bool, pub connected: bool }

    fn s(v: Option<&OwnedValue>) -> Option<String> { v.and_then(|v| String::try_from(&**v).ok()) }
    fn b(v: Option<&OwnedValue>) -> bool { v.and_then(|v| bool::try_from(&**v).ok()).unwrap_or(false) }
    fn strs(v: Option<&OwnedValue>) -> Vec<String> { v.and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).unwrap_or_default() }
    fn err(e: impl std::fmt::Display) -> String { e.to_string() }

    /// our own pairing agent: the kiosk session has no desktop one, and a Puffco pairs "just works", so every
    /// question gets yes. BlueZ asks the agent of whoever called Pair, which is us.
    struct Agent;
    #[zbus::interface(name = "org.bluez.Agent1")]
    impl Agent {
        fn release(&self) {}
        fn request_pin_code(&self, _d: OwnedObjectPath) -> String { "0000".into() }
        fn display_pin_code(&self, _d: OwnedObjectPath, _pin: String) {}
        fn request_passkey(&self, _d: OwnedObjectPath) -> u32 { 0 }
        fn display_passkey(&self, _d: OwnedObjectPath, _key: u32, _entered: u16) {}
        fn request_confirmation(&self, _d: OwnedObjectPath, _key: u32) {}
        fn request_authorization(&self, _d: OwnedObjectPath) {}
        fn authorize_service(&self, _d: OwnedObjectPath, _uuid: String) {}
        fn cancel(&self) {}
    }
    const AGENT: &str = "/org/cratebox/agent";

    impl Bus {
        pub fn new() -> Result<Bus, String> {
            let conn = zbus::blocking::connection::Builder::system().map_err(err)?.method_timeout(Duration::from_secs(25)).build()
                .map_err(|e| format!("can't reach the Bluetooth service ({e})"))?;
            if conn.object_server().at(AGENT, Agent).is_ok() {
                if let Ok(m) = Proxy::new(&conn, "org.bluez", "/org/bluez", "org.bluez.AgentManager1") {
                    let _: Result<(), _> = m.call("RegisterAgent", &(zbus::zvariant::ObjectPath::try_from(AGENT).unwrap(), "NoInputNoOutput"));
                }
            }
            let mut bus = Bus { conn, adapter: OwnedObjectPath::try_from("/").unwrap() };
            let objs = bus.objects()?;
            let mut ads: Vec<&OwnedObjectPath> = objs.iter().filter(|(_, i)| i.contains_key("org.bluez.Adapter1")).map(|(p, _)| p).collect();
            ads.sort_by_key(|p| p.as_str().to_string());
            let Some(a) = ads.first() else { return Err("no Bluetooth adapter found".into()) };
            bus.adapter = (*a).clone();
            let powered = b(objs[*a]["org.bluez.Adapter1"].get("Powered"));
            if !powered {
                let p = bus.proxy(bus.adapter.as_str(), "org.freedesktop.DBus.Properties")?;
                let r: Result<(), _> = p.call("Set", &("org.bluez.Adapter1", "Powered", Value::from(true)));
                if r.is_err() { return Err("Bluetooth is off".into()); }
            }
            Ok(bus)
        }
        pub fn proxy(&self, path: &str, iface: &'static str) -> Result<Proxy<'static>, String> {
            Proxy::new(&self.conn, "org.bluez", path.to_string(), iface).map_err(err)
        }
        pub fn objects(&self) -> Result<Objects, String> {
            let p = self.proxy("/", "org.freedesktop.DBus.ObjectManager")?;
            p.call("GetManagedObjects", &()).map_err(err)
        }
        pub fn start_scan(&self) -> Result<(), String> {
            let a = self.proxy(self.adapter.as_str(), "org.bluez.Adapter1")?;
            let mut f: HashMap<&str, Value> = HashMap::new();
            f.insert("Transport", Value::from("le"));
            let _: Result<(), _> = a.call("SetDiscoveryFilter", &(f,));
            match a.call::<_, _, ()>("StartDiscovery", &()) {
                Ok(()) => Ok(()),
                Err(e) if e.to_string().contains("InProgress") => Ok(()),
                Err(e) => Err(format!("couldn't start a Bluetooth scan ({e})")),
            }
        }
        pub fn stop_scan(&self) { if let Ok(a) = self.proxy(self.adapter.as_str(), "org.bluez.Adapter1") { let _: Result<(), _> = a.call("StopDiscovery", &()); } }
        /// every device BlueZ knows on this adapter (bonded ones too, even when they're not advertising)
        pub fn devices(&self) -> Vec<Seen> {
            let Ok(objs) = self.objects() else { return vec![] };
            let pre = format!("{}/", self.adapter.as_str());
            objs.iter().filter(|(p, _)| p.as_str().starts_with(&pre)).filter_map(|(p, i)| {
                let d = i.get(DEV)?;
                let name = s(d.get("Name")).or_else(|| s(d.get("Alias"))).unwrap_or_default();
                let uuids = strs(d.get("UUIDs"));
                let mfr = d.get("ManufacturerData").and_then(|v| v.try_clone().ok()).and_then(|v| HashMap::<u16, OwnedValue>::try_from(v).ok()).is_some_and(|m| m.contains_key(&0x0c03));
                let ln = name.to_lowercase();
                let puffco = uuids.iter().any(|u| u.eq_ignore_ascii_case(lorax::SERVICE) || u.eq_ignore_ascii_case(lorax::SERVICE_LEGACY))
                    || ["peak", "prox", "puff", "pivot"].iter().any(|k| ln.starts_with(k)) || mfr;
                Some(Seen { path: p.as_str().to_string(), addr: s(d.get("Address")).unwrap_or_default(), name, puffco, connected: b(d.get("Connected")) })
            }).collect()
        }
    }

    /// one Puffco over GATT: writes to the command characteristic, replies come back as notifications
    pub struct Ble {
        bus: Arc<Bus>, path: String, name: String, addr: String, is_proxy: bool,
        cmd: String, notes: Receiver<Vec<u8>>, lost: Arc<std::sync::atomic::AtomicBool>,
        on: bool, seq: u16, with_resp: bool, fmt: Fmt, live_fmt: Option<Fmt>, temp_path: &'static str, soc: lorax::SocDecoder,
    }
    impl Ble {
        pub fn connect(bus: Arc<Bus>, seen: &Seen, say: &mut dyn FnMut(&str)) -> Result<Ble, String> {
            let dev = bus.proxy(&seen.path, DEV)?;
            let is_proxy = { let n = seen.name.to_lowercase(); n.contains("prox") || n.contains("pivot") };
            // the link often drops right after connecting (worst on Proxy), so connect + discovery are retried together
            let mut chars = None;
            for i in 1..=3u64 {
                let r: Result<(), String> = (|| {
                    dev.call::<_, _, ()>("Connect", &()).map_err(err)?;
                    let t0 = Instant::now();
                    while !dev.get_property::<bool>("ServicesResolved").unwrap_or(false) {
                        if t0.elapsed() > Duration::from_secs(10) { return Err("the device didn't answer".into()); }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    chars = Some(find_chars(&bus, &seen.path)?);
                    Ok(())
                })();
                match r {
                    Ok(()) => break,
                    Err(e) => {
                        let _: Result<(), _> = dev.call("Disconnect", &());
                        if i == 3 { return Err(connect_hint(&e)); }
                        say(&format!("Connecting… (try {} of 3: {e})", i + 1));
                        std::thread::sleep(Duration::from_millis(800 * i));
                    }
                }
            }
            let (cmd, reply, version, flags) = chars.ok_or("no Lorax service")?;
            let mut pair_err: Option<String> = None;
            // a Puffco wants a bond; BlueZ pairs "just works" devices without asking, or the desktop's agent asks
            if !dev.get_property::<bool>("Paired").unwrap_or(false) {
                say("Pairing…");
                if let Err(e) = dev.call::<_, _, ()>("Pair", &()) {
                    let e = e.to_string();
                    // already paired from the other side is fine; anything else is worth knowing about
                    if !e.contains("AlreadyExists") { pair_err = Some(e); }
                }
            }
            let _ = Proxy::new(&bus.conn, "org.bluez", seen.path.clone(), "org.freedesktop.DBus.Properties")
                .and_then(|p| p.call::<_, _, ()>("Set", &(DEV, "Trusted", Value::from(true))));
            let opts: HashMap<&str, Value> = HashMap::new();
            if let Some(v) = &version { if let Ok(p) = bus.proxy(v, CHR) { let _: Result<Vec<u8>, _> = p.call("ReadValue", &(opts,)); } }
            // notifications: replies, and the device's Connected going false
            let (tx, notes) = channel();
            let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
            listen(&bus, &seen.path, &reply, tx, lost.clone())?;
            let rp = bus.proxy(&reply, CHR)?;
            let mut subscribed = false;
            for _ in 0..3 {
                if rp.call::<_, _, ()>("StartNotify", &()).is_ok() { subscribed = true; break; }
                std::thread::sleep(Duration::from_millis(1500));
            }
            if !subscribed { let _: Result<(), _> = dev.call("Disconnect", &()); return Err("couldn't subscribe to the device; pair it in the Bluetooth settings and try again".into()); }
            std::thread::sleep(Duration::from_millis(150));
            let both = flags.iter().any(|f| f == "write") && flags.iter().any(|f| f == "write-without-response");
            let with_resp = flags.iter().any(|f| f == "write") || !flags.iter().any(|f| f == "write-without-response");
            let mut me = Ble { bus, path: seen.path.clone(), name: seen.name.clone(), addr: seen.addr.clone(), is_proxy, cmd, notes, lost, on: true, seq: 0, with_resp,
                fmt: Fmt::F32, live_fmt: None, temp_path: lorax::TEMP, soc: Default::default() };
            // unauthenticated clients get dropped after ~10 s, so the handshake has to land
            let auth_problem = |s: &str| { let s = s.to_lowercase(); s.contains("not paired") || s.contains("insufficient auth") || s.contains("encrypt") || s.contains("notpermitted") };
            let mut why = String::new();
            for i in 1..=3 {
                why = me.auth();
                if why == "ok" || auth_problem(&why) { break; }
                if i == 1 && both { me.with_resp = !me.with_resp; }
                if i == 2 {
                    let _: Result<(), _> = rp.call("StopNotify", &()); std::thread::sleep(Duration::from_millis(300));
                    let _: Result<(), _> = rp.call("StartNotify", &()); std::thread::sleep(Duration::from_millis(300));
                }
            }
            if why != "ok" {
                me.disconnect();
                let pe = pair_err.map(|e| format!(" Pairing said: {e}.")).unwrap_or_default();
                return Err(if auth_problem(&why) { format!("it wouldn't pair ({why}).{pe} Forget it in Bluetooth settings (or `bluetoothctl remove {}`), wake it and try again", seen.addr) }
                    else { format!("handshake failed ({why}).{pe} Close the Puffco app (and Cratebox HEAT on your phone): a Puffco talks to one thing at a time") });
            }
            let b = me.read(&lorax::p_temp(0), 4);
            me.fmt = lorax::detect_format(b.as_deref(), is_proxy);
            Ok(me)
        }
        fn send(&mut self, op: u8, payload: &[u8], timeout: f64) -> Result<(u8, Vec<u8>), String> {
            if !self.connected() { return Err("Device is not connected".into()); }
            self.seq = self.seq.wrapping_add(1);
            if self.seq == 0 { self.seq = 1; }
            let s = self.seq;
            while self.notes.try_recv().is_ok() {}
            let p = self.bus.proxy(&self.cmd, CHR)?;
            let mut opts: HashMap<&str, Value> = HashMap::new();
            opts.insert("type", Value::from(if self.with_resp { "request" } else { "command" }));
            p.call::<_, _, ()>("WriteValue", &(lorax::frame(s, op, payload), opts)).map_err(err)?;
            let until = Instant::now() + Duration::from_secs_f64(timeout);
            loop {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() { return Err("BLE reply timed out".into()); }
                match self.notes.recv_timeout(left) {
                    Ok(b) if b.len() >= 3 && u16::from_le_bytes([b[0], b[1]]) == s => return Ok((b[2], b[3..].to_vec())),
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => return Err("BLE reply timed out".into()),
                    Err(RecvTimeoutError::Disconnected) => { self.on = false; return Err("Device disconnected".into()); }
                }
                if self.lost.load(std::sync::atomic::Ordering::Relaxed) { self.on = false; return Err("Device disconnected".into()); }
            }
        }
        fn up(&mut self) -> bool {
            if self.lost.load(std::sync::atomic::Ordering::Relaxed) { self.on = false; }
            self.on
        }
        fn close(&mut self) {
            self.on = false;
            if let Ok(d) = self.bus.proxy(&self.path, DEV) { let _: Result<(), _> = d.call("Disconnect", &()); }
        }
    }
    /// a failed Connect, in words: the usual cause is another phone or app holding the Puffco
    fn connect_hint(e: &str) -> String {
        let l = e.to_lowercase();
        if l.contains("abort") || l.contains("timeout") || l.contains("didn't answer") || l.contains("in progress") {
            format!("{e}. If your phone's Puffco app (or Cratebox HEAT) is connected to it, close that first, then wake the Puffco and try again")
        } else { e.to_string() }
    }
    /// the device's Lorax characteristics: (command, reply, version, command flags)
    fn find_chars(bus: &Bus, dev: &str) -> Result<(String, String, Option<String>, Vec<String>), String> {
        let objs = bus.objects()?;
        let pre = format!("{dev}/");
        let svc_uuid = |p: &str| objs.iter().find(|(k, _)| k.as_str() == p).and_then(|(_, i)| s(i.get("org.bluez.GattService1")?.get("UUID"))).unwrap_or_default().to_lowercase();
        let (mut cmd, mut reply, mut version, mut flags) = (None, None, None, vec![]);
        for want in [lorax::SERVICE, lorax::SERVICE_LEGACY] {
            for (p, i) in objs.iter().filter(|(p, _)| p.as_str().starts_with(&pre)) {
                let Some(c) = i.get(CHR) else { continue };
                let svc = c.get("Service").and_then(|v| v.try_clone().ok()).and_then(|v| OwnedObjectPath::try_from(v).ok()).map(|p| p.as_str().to_string()).unwrap_or_default();
                if svc_uuid(&svc) != want { continue; }
                match s(c.get("UUID")).unwrap_or_default().to_lowercase().as_str() {
                    lorax::CMD => { cmd = Some(p.as_str().to_string()); flags = strs(c.get("Flags")); }
                    lorax::REPLY => reply = Some(p.as_str().to_string()),
                    lorax::VERSION => version = Some(p.as_str().to_string()),
                    _ => {}
                }
            }
            if cmd.is_some() { break; }
        }
        match (cmd, reply) {
            (Some(c), Some(r)) => Ok((c, r, version, flags)),
            _ if objs.keys().any(|k| k.as_str().starts_with(&pre)) => Err("this isn't a Puffco (no Lorax service)".into()),
            _ => Err("the device's command characteristics are missing".into()),
        }
    }
    /// a thread passing on the reply characteristic's notifications, and noticing the device going away
    fn listen(bus: &Bus, dev: &str, reply: &str, tx: Sender<Vec<u8>>, lost: Arc<std::sync::atomic::AtomicBool>) -> Result<(), String> {
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.freedesktop.DBus.Properties").map_err(err)?
            .member("PropertiesChanged").map_err(err)?.path_namespace(dev.to_string()).map_err(err)?.build();
        let it = MessageIterator::for_match_rule(rule, &bus.conn, Some(64)).map_err(err)?;
        let (dev, reply) = (dev.to_string(), reply.to_string());
        std::thread::Builder::new().name("heat-ble".into()).spawn(move || {
            for m in it {
                let Ok(m) = m else { continue };
                let h = m.header();
                let path = h.path().map(|p| p.as_str().to_string()).unwrap_or_default();
                let Ok((_iface, props, _gone)) = m.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else { continue };
                if path == reply {
                    if let Some(v) = props.get("Value").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<u8>::try_from(v).ok()) {
                        if tx.send(v).is_err() { return; }
                    }
                } else if path == dev && props.get("Connected").is_some_and(|v| bool::try_from(&**v) == Ok(false)) {
                    lost.store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }
        }).map_err(err)?;
        Ok(())
    }

    lorax_device!(Ble);
}

// ===================================================================== the worker and the UI side

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Phase { #[default] Idle, Prep, Preheat, Running, Hold }

#[derive(Clone, Debug)]
pub struct Found { pub path: String, pub addr: String, pub name: String }

/// what the panel and the HEAT button show (the worker keeps it current)
#[derive(Clone, Default)]
pub struct View {
    pub label: String,
    pub dev_text: String,
    pub msg: String,
    pub phase: Phase,
    pub connected: bool,
    pub dev_id: String,
    pub is_sim: bool,
    pub scanning: bool,
    pub found: Vec<Found>,
    pub others: Vec<Found>,
    pub puff: f64,
    pub puff_info: String,
}

enum Cmd { Scan(String), Pick(Found), Sim, Disconnect, Run(String, CurveDef, HeaterModel), Hold(f64, f64), Stop, Hit(f64), Panel(bool) }
/// what the app has to act on
pub enum Event { Connected(bool), Puff(f64), Model(String, HeaterModel), DeviceId(String), Status(String) }

pub struct Heat {
    tx: Sender<Cmd>,
    pub events: Receiver<Event>,
    shared: Arc<Mutex<View>>,
    /// this frame's copy of the worker's view
    pub view: View,
    panel: bool,
}

impl Default for Heat { fn default() -> Self { Heat::new() } }

impl Heat {
    pub fn new() -> Heat {
        let (tx, rx) = channel();
        let (etx, events) = channel();
        let shared = Arc::new(Mutex::new(View { label: "HEAT".into(), dev_text: "No device".into(), ..Default::default() }));
        let w = Worker { rx, ev: etx, shared: shared.clone(), dev: None, bus: None, scan_until: None, auto: String::new(), phase: Phase::Idle, msg: String::new(),
            live: None, target: None, t: None, end: 0.0, battery: None, drop: 0.0, puff: 0.0, puff_info: String::new(), abort: false, panel: false,
            found: vec![], others: vec![], t0: Instant::now() };
        let _ = std::thread::Builder::new().name("heat".into()).spawn(move || w.main());
        let view = shared.lock().map(|v| v.clone()).unwrap_or_default();
        Heat { tx, events, shared, view, panel: false }
    }
    /// every frame (UI thread): refresh the copy of the worker's view
    pub fn poll(&mut self) { if let Ok(v) = self.shared.try_lock() { self.view = v.clone(); } }
    pub fn label(&self) -> String { self.view.label.clone() }
    pub fn lit(&self) -> bool { matches!(self.view.phase, Phase::Preheat | Phase::Running | Phase::Hold) }
    pub fn running(&self) -> bool { self.view.phase != Phase::Idle }
    pub fn scan(&self, remembered: &str) { let _ = self.tx.send(Cmd::Scan(remembered.to_string())); }
    pub fn pick(&self, f: &Found) { let _ = self.tx.send(Cmd::Pick(f.clone())); }
    pub fn use_simulator(&self) { let _ = self.tx.send(Cmd::Sim); }
    pub fn disconnect(&self) { let _ = self.tx.send(Cmd::Disconnect); }
    pub fn run(&self, name: &str, def: CurveDef, model: HeaterModel) { let _ = self.tx.send(Cmd::Run(name.to_string(), def, model)); }
    pub fn hold(&self, f: f64, full: f64) { let _ = self.tx.send(Cmd::Hold(f, full)); }
    pub fn stop(&self) { let _ = self.tx.send(Cmd::Stop); }
    /// the simulator's stand-in for a draw (held = 1)
    pub fn hit(&self, a: f64) { let _ = self.tx.send(Cmd::Hit(a)); }
    pub fn set_panel(&mut self, open: bool) { if open != self.panel { self.panel = open; let _ = self.tx.send(Cmd::Panel(open)); } }
}

struct Worker {
    rx: Receiver<Cmd>, ev: Sender<Event>, shared: Arc<Mutex<View>>,
    dev: Option<Box<dyn Device>>, bus: Option<Arc<ble::Bus>>, scan_until: Option<Instant>, auto: String,
    phase: Phase, msg: String, live: Option<f64>, target: Option<f64>, t: Option<f64>, end: f64, battery: Option<i64>,
    drop: f64, puff: f64, puff_info: String, abort: bool, panel: bool, found: Vec<Found>, others: Vec<Found>, t0: Instant,
}

fn f0(v: Option<f64>) -> String { v.map(|v| format!("{}", v.round() as i64)).unwrap_or_else(|| "--".into()) }

impl Worker {
    fn now(&self) -> f64 { self.t0.elapsed().as_secs_f64() }
    fn say(&mut self, m: &str) {
        self.msg = m.to_string(); self.render();
        // heat.log in the data folder: what HEAT said, for when a connect goes wrong on the real thing
        use std::io::Write;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::store::data_dir().join("heat.log")) { let _ = writeln!(f, "{secs} {m}"); }
    }
    fn render(&mut self) {
        let label = match self.phase {
            Phase::Idle => "HEAT".to_string(),
            Phase::Prep => "HEAT…".into(),
            Phase::Preheat => format!("HEAT {}° ↑{}", f0(self.live), f0(self.target)),
            Phase::Running => format!("HEAT {}° {}/{}s", f0(self.live), self.t.unwrap_or(0.0) as i64, self.end.round() as i64),
            Phase::Hold => format!("HOLD {}°{}", f0(self.live), if self.drop >= 1.0 { format!(" ↓{}", self.drop.round() as i64) } else { String::new() }),
        };
        let bat = self.battery.map(|b| format!(" · {b}%")).unwrap_or_default();
        let (dev_text, connected, dev_id, is_sim) = match &self.dev {
            Some(d) => (format!("{}{bat} · bowl {}°F", d.name(), f0(self.live)), true, d.id(), d.is_sim()),
            None => ("No device".to_string(), false, String::new(), false),
        };
        if let Ok(mut v) = self.shared.lock() {
            *v = View { label, dev_text, msg: self.msg.clone(), phase: self.phase, connected, dev_id, is_sim, scanning: self.scan_until.is_some(),
                found: self.found.clone(), others: self.others.clone(), puff: self.puff, puff_info: self.puff_info.clone() };
        }
    }
    fn main(mut self) {
        let mut last_idle = Instant::now() - Duration::from_secs(10);
        let mut n = 0u64;
        loop {
            let wait = if self.scan_until.is_some() { Duration::from_millis(700) } else { Duration::from_millis(300) };
            match self.rx.recv_timeout(wait) {
                Ok(c) => self.handle(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => { self.disconnect(); return; }
            }
            if self.scan_until.is_some() { self.scan_tick(); }
            // bowl temperature and battery while no curve runs (less often when the panel is closed)
            let every = if self.panel { 1 } else { 3 };
            if self.dev.is_some() && last_idle.elapsed() >= Duration::from_secs(every) {
                last_idle = Instant::now();
                if !self.check_link() { continue; }
                let d = self.dev.as_mut().unwrap();
                self.live = d.read_temp();
                if n % 30 == 0 { self.battery = d.read_battery(); }
                n += 1;
                self.render();
            }
        }
    }
    /// false (and tidied up) if the device has gone
    fn check_link(&mut self) -> bool {
        let Some(d) = self.dev.as_mut() else { return false };
        if d.connected() { return true; }
        let running = self.phase != Phase::Idle;
        self.dev = None;
        let _ = self.ev.send(Event::Connected(false));
        if running { self.abort = true; }
        self.say(if running { "Connection lost mid-curve; the device finishes its own cycle at its last temperature." } else { "Disconnected." });
        false
    }
    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Scan(auto) => self.scan(auto),
            Cmd::Pick(f) => self.pick(f),
            Cmd::Sim => { self.stop_scan(); self.connected(Box::new(SimDevice::new())); }
            Cmd::Disconnect => self.disconnect(),
            Cmd::Run(name, def, model) => self.run(&name, &def, model),
            Cmd::Hold(f, full) => self.hold(f, full),
            Cmd::Stop | Cmd::Hit(_) => { if let (Cmd::Hit(a), Some(d)) = (&c, self.dev.as_mut()) { d.set_airflow(*a); } }
            Cmd::Panel(p) => self.panel = p,
        }
    }
    /// during a run: Stop, the simulator's hit, the panel, a disconnect; anything else waits its turn
    fn check(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(Cmd::Stop) => { self.abort = true; self.say("Stopping…"); }
                Ok(Cmd::Disconnect) => { self.abort = true; }
                Ok(Cmd::Hit(a)) => { if let Some(d) = self.dev.as_mut() { d.set_airflow(a); } }
                Ok(Cmd::Panel(p)) => self.panel = p,
                Ok(_) => self.say("Stop the heat run first."),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => { self.abort = true; return; }
            }
        }
    }

    // ---- connecting ----
    fn scan(&mut self, auto: String) {
        if self.dev.is_some() { return; }
        self.found.clear(); self.others.clear();
        self.auto = auto;
        if self.bus.is_none() {
            match ble::Bus::new() { Ok(b) => self.bus = Some(Arc::new(b)), Err(e) => { self.say(&format!("Bluetooth: {e}. Try the simulator.")); return; } }
        }
        let bus = self.bus.clone().unwrap();
        if let Err(e) = bus.start_scan() { self.bus = None; self.say(&e); return; }
        self.scan_until = Some(Instant::now() + Duration::from_secs(15));
        self.say("Looking for your Puffco… (wake it up if it's asleep)");
        self.scan_tick();
    }
    fn scan_tick(&mut self) {
        let Some(bus) = self.bus.clone() else { self.scan_until = None; return };
        for s in bus.devices() {
            let f = Found { path: s.path.clone(), addr: s.addr.clone(), name: if s.name.is_empty() { "Puffco".into() } else { s.name.clone() } };
            if !s.puffco {
                if !s.name.is_empty() && self.others.len() < 12 && !self.others.iter().any(|o| o.path == s.path) { self.others.push(f); }
                continue;
            }
            if self.found.iter().any(|o| o.path == s.path) { continue; }
            self.others.retain(|o| o.path != s.path);
            self.found.push(f.clone());
            if (!self.auto.is_empty() && s.addr == self.auto) || s.connected { self.pick(f); return; }
            if self.found.len() == 1 { let m = format!("Found {}. Click it to connect.", f.name); self.say(&m); }
        }
        self.render();
        if self.scan_until.is_some_and(|t| Instant::now() >= t) {
            self.stop_scan();
            let m = if !self.found.is_empty() { "Pick your device." } else if self.others.is_empty() { "No Bluetooth devices found. Is Bluetooth on, and the Puffco awake?" }
                else { "No Puffco recognised. If yours has its own name, pick it from the list below." };
            self.say(m);
        }
    }
    fn stop_scan(&mut self) { if self.scan_until.take().is_some() { if let Some(b) = &self.bus { b.stop_scan(); } } }
    fn pick(&mut self, f: Found) {
        if self.dev.is_some() { return; }
        self.stop_scan();
        let Some(bus) = self.bus.clone() else { return };
        self.say(&format!("Connecting to {}… (accept the pairing prompt if one comes up)", f.name));
        let seen = ble::Seen { path: f.path.clone(), addr: f.addr.clone(), name: f.name.clone(), puffco: true, connected: false };
        let shared = self.shared.clone();
        let mut note = |m: &str| { if let Ok(mut v) = shared.lock() { v.msg = m.to_string(); } };
        match ble::Ble::connect(bus, &seen, &mut note) {
            Ok(b) => { let _ = self.ev.send(Event::DeviceId(f.addr.clone())); self.connected(Box::new(b)); }
            Err(e) => self.say(&format!("Couldn't connect: {e}")),
        }
    }
    fn connected(&mut self, d: Box<dyn Device>) {
        let name = d.name();
        self.dev = Some(d);
        self.found.clear();
        let _ = self.ev.send(Event::Connected(true));
        self.say(&format!("Connected to {name}."));
    }
    fn disconnect(&mut self) {
        if let Some(mut d) = self.dev.take() { d.disconnect(); let _ = self.ev.send(Event::Connected(false)); self.say("Disconnected."); }
    }

    // ---- running a curve ----
    fn run(&mut self, name: &str, def: &CurveDef, model: HeaterModel) {
        if self.dev.is_none() { return; }
        let Some(curve) = Curve::new(&def.points, def.smooth) else { self.say("That curve has no length."); return };
        let mut drv = Driver::new(&curve, model);
        let mut learner = Learner::new(model);
        let (mut backup, mut orig_slot, mut completed, mut err_sum, mut err_n) = (None, None, false, 0.0, 0u32);
        self.abort = false; self.phase = Phase::Prep; self.t = None; self.end = curve.end; self.target = Some(curve.at(0.0));
        self.say(&format!("Setting up \"{name}\"…"));
        let r: Result<(), String> = (|| {
            let d = self.dev.as_mut().ok_or("no device")?;
            if heating(d.read_state()) { d.stop(); std::thread::sleep(Duration::from_millis(1500)); }
            orig_slot = d.read_active_slot();
            backup = Some(d.backup_slot(SLOT));
            let start_f = curve.at(0.0).round();
            if !d.write_profile_temp(SLOT, start_f) { return Err("the device refused the start temperature".into()); }
            d.write_profile_time(SLOT, (curve.end + 10.0).ceil().min(120.0));
            d.set_active_slot(SLOT);
            if !d.start() { return Err("the device didn't accept the start command".into()); }
            self.phase = Phase::Preheat; self.target = Some(start_f);
            self.say(&format!("Preheating to {start_f}°F. Back to the decks; it'll run on its own."));
            let p0 = self.now();
            while !self.abort {
                self.check();
                if !self.check_link() { return Ok(()); }
                let d = self.dev.as_mut().unwrap();
                let y = d.read_temp();
                let st = d.read_state();
                self.live = y;
                let tp = self.now() - p0;
                self.render();
                if let Some(y) = y { if y >= CUTOFF_F { return Err(format!("overheat cutoff at {y}°F")); } }
                // some firmware (Proxy) says "heating" long before the bowl is hot, so the temperature has to agree too
                if let Some(y) = y { if y >= start_f - 8.0 || (heating(st) && st != Some(7) && y >= start_f - 30.0) { break; } }
                if tp > 6.0 && !heating(st) { return Err(format!("the device stopped heating during preheat (at {}°F)", f0(y))); }
                if tp > 70.0 { break; }
                std::thread::sleep(Duration::from_millis(250));
            }
            if self.abort { return Ok(()); }
            self.dev.as_mut().ok_or("no device")?.probe_live_format();
            self.phase = Phase::Running;
            self.say(&format!("Running \"{name}\"."));
            let t0 = self.now();
            let (mut last, mut sent, mut sent_at, mut tick) = (t0, start_f, 0.0, 0u64);
            while !self.abort {
                self.check();
                if !self.check_link() { return Ok(()); }
                let tn = self.now();
                let (t, dt) = (tn - t0, tn - last);
                last = tn;
                if t >= curve.end { completed = true; break; }
                let d = self.dev.as_mut().unwrap();
                let y = d.read_temp();
                if tick % 3 == 0 && t > 2.0 && !heating(d.read_state()) { self.say(&format!("The device ended its heat cycle {} s in.", t.round())); break; }
                tick += 1;
                if let Some(y) = y { if y >= CUTOFF_F { return Err(format!("overheat cutoff at {y}°F")); } }
                let u = (drv.command(t, y, dt) * 10.0).round() / 10.0;
                let diff = (u - sent).abs();
                let d = self.dev.as_mut().unwrap();
                if (diff >= 1.5 || (diff >= 0.5 && tn - sent_at > 1.5)) && d.set_live_target(u) { sent = u; sent_at = tn; }
                learner.add(t, y, sent);
                let r = curve.at(t);
                self.t = Some(t); self.live = y; self.target = Some(r);
                if let Some(y) = y { if t > 1.0 { err_sum += (y - r).abs(); err_n += 1; } }
                self.render();
                let spent = self.now() - tn;
                std::thread::sleep(Duration::from_secs_f64((0.2 - spent).max(0.0)));
            }
            Ok(())
        })();
        if let Err(e) = r { self.say(&format!("Stopped: {e}")); }
        self.finish(backup, orig_slot);
        if err_n > 10 {
            self.say(&format!("{}: \"{name}\", {:.1}°F average off the curve.", if completed { "Done" } else { "Stopped" }, err_sum / err_n as f64));
            if learner.n > 15 {
                if let Some(d) = &self.dev {
                    let mut m = learner.m;
                    m.dead = model.dead; m.runs = model.runs + 1;
                    let _ = self.ev.send(Event::Model(d.id(), m));
                }
            }
        } else if self.abort && self.dev.is_some() { self.say("Stopped."); }
        self.phase = Phase::Idle; self.t = None; self.target = None;
        self.render();
        let _ = self.ev.send(Event::Status(format!("Heat curve \"{name}\" {}", if completed { "done" } else { "stopped" })));
    }
    /// stop the device and put slot 4 and the active slot back
    fn finish(&mut self, backup: Option<SlotBackup>, orig_slot: Option<u8>) {
        if let Some(d) = self.dev.as_mut() {
            if d.connected() {
                d.stop();
                if let Some(b) = &backup { d.restore_slot(SLOT, b); }
                if let Some(s) = orig_slot { d.set_active_slot(s); }
            }
        }
    }

    // ---- HOLD: one temperature, played with PUFF ----
    /// Same setup and safety as a curve, but the target is written once and never steered: the device's own
    /// regulator holds it, and the dip on a draw is what PUFF plays. One device heat cycle (120 s).
    fn hold(&mut self, hold_f: f64, full: f64) {
        if self.dev.is_none() { return; }
        let f = clamp(hold_f.round(), MIN_F, MAX_F);
        let (mut backup, mut orig_slot, mut ended) = (None, None, false);
        self.abort = false; self.phase = Phase::Prep; self.t = None; self.target = Some(f); self.puff = 0.0; self.drop = 0.0;
        self.say(&format!("Setting up HOLD at {f}°F…"));
        let mut meter = PuffDrop::new(f, full);
        let r: Result<(), String> = (|| {
            let d = self.dev.as_mut().ok_or("no device")?;
            if heating(d.read_state()) { d.stop(); std::thread::sleep(Duration::from_millis(1500)); }
            orig_slot = d.read_active_slot();
            backup = Some(d.backup_slot(SLOT));
            if !d.write_profile_temp(SLOT, f) { return Err("the device refused the temperature".into()); }
            d.write_profile_time(SLOT, 120.0);
            d.set_active_slot(SLOT);
            if !d.start() { return Err("the device didn't accept the start command".into()); }
            self.phase = Phase::Hold;
            self.say(&format!("Holding {f}°F. Pick PUFF in Beat FX and turn it on; hit it once the bowl is up to temperature."));
            let t0 = self.now();
            let (mut tick, mut reads, mut rate_at) = (0u64, 0u32, t0);
            while !self.abort {
                self.check();
                if !self.check_link() { return Ok(()); }
                let tn = self.now();
                let tt = tn - t0;
                let d = self.dev.as_mut().unwrap();
                let y = d.read_temp();
                if tick % 5 == 0 && tt > 6.0 && !heating(d.read_state()) { ended = true; break; }
                tick += 1;
                if let Some(y) = y { if y >= CUTOFF_F { return Err(format!("overheat cutoff at {y}°F")); } }
                self.live = y;
                if y.is_some() { reads += 1; }
                if tn - rate_at >= 2.0 {
                    self.puff_info = format!("dip {:.1}°F · biggest {:.1} · full at {:.0} · {:.1} reads/s", meter.drop, meter.peak, meter.scale(), reads as f64 / (tn - rate_at));
                    reads = 0; rate_at = tn;
                }
                let a = meter.add(tt, y);
                self.drop = meter.drop;
                if (a - self.puff).abs() > 0.005 || (a == 0.0 && self.puff != 0.0) { self.puff = a; let _ = self.ev.send(Event::Puff(a)); }
                self.render();
                // as fast as the link answers, at most ~10 a second
                let spent = self.now() - tn;
                std::thread::sleep(Duration::from_secs_f64((0.1 - spent).max(0.0)));
            }
            Ok(())
        })();
        if let Err(e) = r { self.say(&format!("Stopped: {e}")); }
        self.puff = 0.0; self.drop = 0.0; self.puff_info.clear();
        let _ = self.ev.send(Event::Puff(0.0));
        self.finish(backup, orig_slot);
        if ended { self.say("The device ended its heat cycle. Hold again for another round."); }
        else if self.abort && self.dev.is_some() { self.say("Stopped."); }
        self.phase = Phase::Idle; self.target = None;
        self.render();
        let _ = self.ev.send(Event::Status(if ended { "HOLD: heat cycle over".into() } else { "HOLD stopped".into() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn banger() -> CurveDef { preset("Quartz Banger").unwrap() }
    fn hex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() }
    fn near(a: f64, b: f64, e: f64) { assert!((a - b).abs() <= e, "{a} vs {b}"); }

    // expected numbers from cratebox.html's TC code in node, same as the iOS tests
    #[test]
    fn curve_matches_web() {
        let c = Curve::new(&banger().points, true).unwrap();
        assert_eq!(c.end, 65.0);
        for (t, f) in [(0.0, 520.0), (10.0, 508.0), (25.0, 485.0), (45.0, 462.0), (65.0, 450.0), (5.0, 515.669), (30.0, 478.424)] { near(c.at(t), f, 0.001); }
        near(c.slope(5.0), -1.4661, 1e-4);
        near(c.slope(30.0), -1.2954, 1e-4);
        let lin = Curve::new(&banger().points, false).unwrap();
        near(lin.at(5.0), 514.0, 0.001);
        near(lin.at(30.0), 479.25, 0.001);
        assert_eq!(c.at(-5.0), 520.0);
        assert_eq!(c.at(99.0), 450.0);
        assert!(Curve::new(&[Point { t: 3.0, f: 500.0 }, Point { t: 3.0, f: 510.0 }], true).is_none());
    }
    #[test]
    fn curve_never_overshoots() {
        for (name, _) in PRESETS {
            let def = preset(name).unwrap();
            let c = Curve::new(&def.points, true).unwrap();
            for w in def.points.windows(2) {
                let (lo, hi) = (w[0].f.min(w[1].f) - 1e-9, w[0].f.max(w[1].f) + 1e-9);
                for k in 0..=20 { let v = c.at(w[0].t + (w[1].t - w[0].t) * k as f64 / 20.0); assert!(v >= lo && v <= hi, "{name} {v}"); }
            }
        }
    }
    #[test]
    fn driver_stays_in_safe_band() {
        let c = Curve::new(&banger().points, true).unwrap();
        let mut d = Driver::new(&c, HeaterModel::default());
        for k in 0..400 {
            let t = k as f64 * 0.2;
            let r = c.at(t);
            for y in [None, Some(70.0), Some(r - 50.0), Some(r), Some(r + 50.0), Some(640.0)] {
                let u = d.command(t, y, 0.2);
                assert!(u >= MIN_F.max(r - 40.0) - 1e-9 && u <= MAX_F.min(r + 30.0) + 1e-9);
            }
        }
    }
    /// the runner's loop against the simulator: preheat, then 5 Hz commands with the web app's send rules
    #[test]
    fn closed_loop_tracks_quartz_banger() {
        let c = Curve::new(&banger().points, true).unwrap();
        let mut sim = SimHeater::default();
        let mut drv = Driver::new(&c, HeaterModel::default());
        let mut learner = Learner::new(HeaterModel::default());
        sim.slots[3] = (520.0, 75.0); sim.slot_idx = 3; sim.start();
        let adv = |sim: &mut SimHeater, dt: f64| for _ in 0..(dt / 0.05).round() as i32 { sim.advance(0.05); };
        let mut pt = 0.0;
        while sim.y < 512.0 && pt < 70.0 { adv(&mut sim, 0.25); pt += 0.25; }
        assert!(pt < 60.0, "preheat");
        let (mut sent, mut sent_at, mut err, mut n, mut worst, mut t) = (520.0, -9.0, 0.0, 0.0, 0.0f64, 0.0);
        while t < c.end {
            let y = (sim.y * 10.0).round() / 10.0;
            let u = (drv.command(t, Some(y), 0.2) * 10.0).round() / 10.0;
            let diff = (u - sent).abs();
            if diff >= 1.5 || (diff >= 0.5 && t - sent_at > 1.5) { sim.set_live_target(u); sent = u; sent_at = t; }
            learner.add(t, Some(y), sent);
            if t > 1.0 { let e = (y - c.at(t)).abs(); err += e; n += 1.0; worst = worst.max(e); }
            assert!(y < CUTOFF_F);
            adv(&mut sim, 0.2); t += 0.2;
        }
        // node gives 0.27 average, 4.2 worst
        assert!(err / n < 2.0, "{}", err / n);
        assert!(worst < 10.0, "{worst}");
        assert!(heating(Some(sim.state)), "the device should still be in its cycle");
        assert!(learner.m.tau.is_finite() && learner.m.heat_max.is_finite() && learner.m.cool_k.is_finite());
    }
    #[test]
    fn paste_from_truecurve() {
        let txt = r#" {"Mine": {"points": [{"t": 0, "f": 700}, {"t": "20", "f": 480}, {"t": 999, "f": 300}, {"t": "x", "f": 1}], "smooth": false}, "Bad": {"points": [{"t": 0, "f": 500}]}, "A name that is far too long for the menu": {"points": [{"t": 0, "f": 500}, {"t": 9, "f": 510}]}} "#;
        let got = parse_curves(txt).unwrap();
        assert_eq!(got["Mine"], CurveDef { points: vec![Point { t: 0.0, f: 590.0 }, Point { t: 20.0, f: 480.0 }, Point { t: 110.0, f: 350.0 }], smooth: false });
        assert!(!got.contains_key("Bad"));
        assert!(got["A name that is far too l"].smooth);
        assert_eq!(got.len(), 2);
        assert!(parse_curves("not json").is_none());
    }
    #[test]
    fn lorax_bytes() {
        assert_eq!(lorax::token(&(0..16).collect::<Vec<u8>>()), hex("cc4af4246a24eb8bb512bc7dc5157b9c"));
        assert_eq!(lorax::encode_c(f_to_c(520.0), Fmt::F32), hex("398e8743"));
        assert_eq!(lorax::encode_c(f_to_c(520.0), Fmt::I10), hex("970a0000"));
        near(lorax::decode_c(Some(&hex("398e8743")), Some(Fmt::F32)).unwrap(), 271.111, 0.001);
        assert_eq!(lorax::decode_c(Some(&hex("970a0000")), Some(Fmt::I10)), Some(271.1));
        assert_eq!(lorax::decode_c(Some(&hex("970a0000")), None), Some(271.1)); // a tiny denormal float: read as tenths
        let mut want = vec![0, 0, 4, 0]; want.extend_from_slice(b"/p/x");
        assert_eq!(lorax::read_payload("/p/x", 4), want);
        let mut want = vec![0, 0, 0]; want.extend_from_slice(b"/p/x"); want.extend_from_slice(&[0, 9]);
        assert_eq!(lorax::write_payload("/p/x", &[9]), want);
        assert_eq!(lorax::frame(0x0102, 16, &[7]), vec![2, 1, 16, 7]);
        assert_eq!(lorax::detect_format(Some(&hex("398e8743")), true), Fmt::F32);
        assert_eq!(lorax::detect_format(Some(&hex("970a0000")), false), Fmt::I10);
        assert_eq!(lorax::detect_format(None, true), Fmt::I10);
        let mut soc = lorax::SocDecoder::default();
        assert_eq!(soc.decode(Some(&lorax::f32_bytes(0.87))), Some(87));
        assert_eq!(soc.decode(Some(&[0x12, 0x02, 0, 0])), Some(53)); // 530 = 53.0 %
        assert_eq!(soc.decode(Some(&[53])), Some(5)); // the scale sticks once seen
    }
    /// PUFF: a HOLD at 500°F on the simulator; resting reads nothing, a hard draw reads a lot, letting go lets go
    #[test]
    fn puff_drop_follows_a_draw() {
        let mut h = SimHeater::default();
        let mut m = PuffDrop::new(500.0, 0.0);
        h.slots[0] = (500.0, 120.0); h.y = 495.0; h.start();
        let mut rng = 12345u64;
        let mut noise = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; ((rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 1.2 };
        let (mut t, mut peak, mut rest, mut after) = (0.0, 0.0f64, 0.0f64, 1.0f64);
        while t < 25.0 {
            h.airflow = if (10.0..14.0).contains(&t) { 1.0 } else { 0.0 };
            for _ in 0..10 { h.advance(0.01); }
            t += 0.1;
            let a = m.add(t, Some(h.y + noise()));
            if t < 10.0 && t > 2.0 { rest = rest.max(a); }
            if (10.0..14.0).contains(&t) { peak = peak.max(a); }
            if t > 17.0 { after = after.min(a); }
        }
        assert_eq!(rest, 0.0, "noise at rest isn't a hit");
        assert!(peak > 0.9, "{peak}");
        assert_eq!(after, 0.0, "the warble lets go once the draw stops");
    }
    /// the worker against the simulator, start to finish: HOLD, a hit, stop; slot 4 is put back after
    #[test]
    fn worker_holds_on_the_simulator() {
        let mut h = Heat::new();
        h.use_simulator();
        let t0 = Instant::now();
        while !h.view.connected && t0.elapsed() < Duration::from_secs(3) { std::thread::sleep(Duration::from_millis(20)); h.poll(); }
        assert!(h.view.connected && h.view.is_sim);
        h.hold(500.0, 0.0);
        while h.view.phase != Phase::Hold && t0.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(20)); h.poll(); }
        assert_eq!(h.view.phase, Phase::Hold);
        assert!(h.lit() && h.label().starts_with("HOLD"));
        h.stop();
        while h.view.phase != Phase::Idle && t0.elapsed() < Duration::from_secs(8) { std::thread::sleep(Duration::from_millis(20)); h.poll(); }
        assert_eq!(h.view.phase, Phase::Idle);
        let evs: Vec<Event> = h.events.try_iter().collect();
        assert!(evs.iter().any(|e| matches!(e, Event::Connected(true))));
        assert!(evs.iter().any(|e| matches!(e, Event::Status(s) if s.contains("HOLD"))));
    }
}
