//! Controllers: Pioneer DJ / AlphaTheta profiles, a table-driven MIDI engine, LEDs and MIDI learn.
//! A mapping entry is (status, data1, action, deck, lsb): status/data1 of the message (note-offs are
//! treated as their note-on), the Cratebox action, the deck (0/1, -1 = global) and, for 14-bit
//! controls, the data1 of the LSB message (0 if none).

use crate::deck::{JUMPS, LOOP_SIZES};
use crate::maps::mixxx_map;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry(pub u8, pub u8, pub String, pub i8, pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src { Tested, Mixxx, Family, Learn }

pub struct Profile {
    pub id: &'static str,
    pub name: &'static str,
    pub src: Src,
    pub keep_alive: Option<&'static [u8]>,
    pub init: Option<&'static [u8]>,
    pub vu: bool,
    pub leds: bool,
}

pub const FLX4_KEEPALIVE: &[u8] = &[0xf0, 0x00, 0x40, 0x05, 0x00, 0x00, 0x04, 0x05, 0x00, 0x50, 0x02, 0xf7];

pub fn profiles() -> Vec<Profile> {
    let p = |id, name, src, vu, leds| Profile { id, name, src, keep_alive: None, init: None, vu, leds };
    let mut v = vec![
        Profile { keep_alive: Some(FLX4_KEEPALIVE), ..p("ddj-flx4", "Pioneer DDJ-FLX4", Src::Tested, true, true) },
        Profile { init: Some(&[0xf0, 0x00, 0x40, 0x05, 0x00, 0x00, 0x02, 0x06, 0x00, 0x03, 0x01, 0xf7]), ..p("ddj-400", "Pioneer DDJ-400", Src::Mixxx, true, true) },
        Profile { init: Some(&[0xf0, 0x00, 0x40, 0x05, 0x00, 0x00, 0x02, 0x0a, 0x00, 0x03, 0x01, 0xf7]), ..p("ddj-200", "Pioneer DDJ-200", Src::Mixxx, false, true) },
        Profile { init: Some(&[0xf0, 0x00, 0x20, 0x7f, 0x03, 0x01, 0xf7]), ..p("ddj-sb3", "Pioneer DDJ-SB3", Src::Mixxx, false, true) },
        p("ddj-sb2", "Pioneer DDJ-SB2", Src::Mixxx, false, true),
        p("ddj-sb", "Pioneer DDJ-SB", Src::Mixxx, false, true),
        p("ddj-sx2", "Pioneer DDJ-SX2 / SX3", Src::Mixxx, false, true),
        p("ddj-sx", "Pioneer DDJ-SX", Src::Mixxx, false, true),
        p("xdj-rx", "Pioneer XDJ-RX", Src::Mixxx, false, false),
        p("cdj-2000", "Pioneer CDJ-2000 / NXS / NXS2 (MIDI mode)", Src::Mixxx, false, false),
        p("cdj-850", "Pioneer CDJ-850 / 900 (MIDI mode)", Src::Mixxx, false, false),
        p("cdj-350", "Pioneer CDJ-350 (MIDI mode)", Src::Mixxx, false, false),
        p("ddj-rekordbox", "Pioneer DDJ / XDJ, rekordbox series (FLX2, FLX6, FLX10, 800, 1000, GRV6, RB, RR, RX, RZ...)", Src::Family, false, true),
        p("ddj-serato", "Pioneer DDJ, Serato series (SR, SR2, SZ, SZ2, REV1/5/7, 1000SRT, XP)", Src::Family, false, true),
        p("cdj", "Pioneer CDJ / XDJ player (MIDI mode)", Src::Family, false, false),
        p("learn", "Other controller (map it with MIDI learn)", Src::Learn, false, true),
    ];
    v.shrink_to_fit();
    v
}

pub fn src_note(s: Src) -> &'static str {
    match s {
        Src::Tested => "Mapping checked on the hardware.",
        Src::Mixxx => "Mapping from Mixxx's community mapping for this model. Not tested here: if a control is wrong, fix it with Learn.",
        Src::Family => "This model uses the shared Pioneer layout for its product line. The main controls should work; fix anything wrong with Learn.",
        Src::Learn => "No built-in mapping: use Learn to assign controls.",
    }
}

/// uppercase, dashes removed: "DDJ-FLX4 MIDI 1" -> "DDJFLX4 MIDI 1"
fn norm(s: &str) -> String { s.to_uppercase().replace('-', "") }
/// `pat` occurs and is followed by a non-alphanumeric character (or the end): regex `pat\b`
fn has_word(n: &str, pat: &str) -> bool {
    let mut from = 0;
    while let Some(i) = n[from..].find(pat) {
        let end = from + i + pat.len();
        if n[end..].chars().next().map_or(true, |c| !c.is_ascii_alphanumeric()) { return true; }
        from = from + i + 1;
    }
    false
}
fn matches(id: &str, name: &str) -> bool {
    let n = norm(name);
    let c = |p: &str| n.contains(p);
    match id {
        "ddj-flx4" => c("FLX4"),
        "ddj-400" => c("DDJ400"),
        "ddj-200" => c("DDJ200"),
        "ddj-sb3" => c("DDJSB3"),
        "ddj-sb2" => c("DDJSB2"),
        "ddj-sb" => has_word(&n, "DDJSB"),
        "ddj-sx2" => c("DDJSX2") || c("DDJSX3"),
        "ddj-sx" => has_word(&n, "DDJSX"),
        "xdj-rx" => has_word(&n, "XDJRX"),
        "cdj-2000" => c("CDJ2000"),
        "cdj-850" => c("CDJ850") || c("CDJ900"),
        "cdj-350" => c("CDJ350"),
        "ddj-rekordbox" => c("DDJ") || c("FLX") || c("XDJ") || c("OPUS") || c("GRV") || c("ALPHATHETA"),
        "ddj-serato" => ["DDJSR", "DDJSZ", "DDJREV", "DDJXP", "DDJ1000SRT"].iter().any(|p| c(p)),
        "cdj" => c("CDJ") || c("XDJ1000") || c("XDJ700"),
        _ => false,
    }
}
pub fn pioneerish(name: &str) -> bool {
    let n = norm(name);
    ["DDJ", "FLX", "CDJ", "XDJ", "DJM", "OPUS", "GRV", "PIONEER", "ALPHATHETA"].iter().any(|p| n.contains(p))
}
/// specific models first, then the product-line fallbacks (last listed first), then learn
pub fn profile_for(port: &str) -> &'static str {
    let ps = profiles();
    if let Some(p) = ps.iter().find(|p| p.src != Src::Family && matches(p.id, port)) { return p.id; }
    if let Some(p) = ps.iter().filter(|p| p.src == Src::Family).rev().find(|p| matches(p.id, port)) { return p.id; }
    "learn"
}

/// The rekordbox-era DDJ layout (DDJ-400, FLX2/4/6/10, 800, 1000...), written out from the FLX4,
/// which it was verified on. deck_chs: the MIDI channels the decks use (4-deck units fold 3/4 onto 1/2).
pub fn ddj_rekordbox_map(deck_chs: &[u8], pad_chs: &[(u8, u8)]) -> Vec<Entry> {
    let mut m = vec![];
    let e = |st: u8, no: u8, a: &str, d: i8, l: u8| Entry(st, no, a.to_string(), d, l);
    for (k, &ch) in deck_chs.iter().enumerate() {
        let d = (k % 2) as i8;
        let (n, c) = (0x90 + ch, 0xb0 + ch);
        let notes: [(u8, &str); 30] = [(0x0b, "play"), (0x0e, "playCue"), (0x0c, "cue"), (0x48, "start"), (0x58, "sync"), (0x5c, "syncOn"),
            (0x60, "range"), (0x36, "touch"), (0x67, "touch"), (0x10, "loopIn"), (0x11, "loopOut"), (0x4d, "reloop"),
            (0x50, "reloopStop"), (0x4c, "loopHalf"), (0x4e, "loopDouble"), (0x51, "callPrev"), (0x53, "callNext"),
            (0x3e, "jump:-4"), (0x3d, "jump:4"), (0x54, "pfl"), (0x68, "quant"), (0x3f, "ignore"),
            (0x1b, "mode:hotcue"), (0x6d, "mode:beatloop"), (0x20, "mode:beatjump"), (0x1e, "mode:padfx"),
            (0x6b, "mode:padfx2"), (0x22, "mode:sampler"), (0x69, "mode:keyboard"), (0x6f, "mode:keyshift")];
        for (no, a) in notes { m.push(e(n, no, a, d, 0)); }
        for (no, a) in [(0x22, "jogTop"), (0x23, "jogTop"), (0x21, "jogSide"), (0x29, "jogSearch"), (0x02, "ignore")] { m.push(e(c, no, a, d, 0)); }
        for (no, a) in [(0x00u8, "tempo"), (0x13, "fader"), (0x04, "trim"), (0x07, "hi"), (0x0b, "mid"), (0x0f, "low")] { m.push(e(c, no, a, d, no + 0x20)); }
    }
    for (k, &(pn, ps)) in pad_chs.iter().enumerate() {
        let d = (k % 2) as i8;
        for p in 0..8u8 {
            let ls = fmt_num(LOOP_SIZES[p as usize]);
            m.push(e(pn, p, &format!("hot:{p}"), d, 0));
            m.push(e(ps, p, &format!("hotClear:{p}"), d, 0));
            m.push(e(pn, 0x60 + p, &format!("loop:{ls}"), d, 0));
            m.push(e(ps, 0x60 + p, &format!("loop:{ls}"), d, 0));
            m.push(e(pn, 0x20 + p, &format!("jump:{}", fmt_num(JUMPS[p as usize])), d, 0));
        }
    }
    for x in [(0x96, 0x46, "load", 0, 0), (0x96, 0x47, "load", 1, 0), (0x96, 0x48, "load", 0, 0), (0x96, 0x49, "load", 1, 0),
              (0x96, 0x41, "browsePress", -1, 0), (0x96, 0x42, "browseShift", -1, 0), (0xb6, 0x40, "browse", -1, 0), (0xb6, 0x64, "zoom", -1, 0),
              (0xb6, 0x1f, "xfader", -1, 0x3f), (0xb6, 0x17, "cfx", 0, 0x37), (0xb6, 0x18, "cfx", 1, 0x38), (0xb6, 0x0c, "headMix", -1, 0x2c)] {
        m.push(e(x.0, x.1, x.2, x.3, x.4));
    }
    for st in [0x94u8, 0x95] { for no in 0..128u8 { m.push(e(st, no, "unsupported:Beat FX", -1, 0)); } }
    m
}
fn fmt_num(v: f64) -> String { if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") } }

/// Mixxx maps only cover channels 1-2 for some units: copy them onto 3-4 for four-deck hardware
fn with_decks34(map: Vec<Entry>) -> Vec<Entry> {
    let have: std::collections::HashSet<u16> = map.iter().map(|e| (e.0 as u16) << 8 | e.1 as u16).collect();
    let mut out = map.clone();
    for e in &map {
        let (t, ch) = (e.0 & 0xf0, e.0 & 0x0f);
        if (t == 0x90 || t == 0xb0) && ch <= 1 {
            let st = e.0 + 2;
            if !have.contains(&((st as u16) << 8 | e.1 as u16)) { out.push(Entry(st, e.1, e.2.clone(), e.3, e.4)); }
        }
    }
    out
}
fn mixxx(id: &str) -> Vec<Entry> { mixxx_map(id).iter().map(|e| Entry(e.0, e.1, e.2.to_string(), e.3, e.4)).collect() }

pub fn profile_map(id: &str) -> Vec<Entry> {
    let flx = || ddj_rekordbox_map(&[0, 1], &[(0x97, 0x98), (0x99, 0x9a)]);
    match id {
        "ddj-flx4" => flx(),
        "ddj-400" => { let mut m = mixxx("ddj-400"); m.extend(flx()); m }
        "ddj-200" | "ddj-sb3" | "ddj-sb2" | "ddj-sx2" | "ddj-sx" | "xdj-rx" => mixxx(id),
        "ddj-sb" | "cdj-2000" | "cdj-850" | "cdj-350" => with_decks34(mixxx(id)),
        "ddj-rekordbox" => ddj_rekordbox_map(&[0, 1, 2, 3], &[(0x97, 0x98), (0x99, 0x9a), (0x9b, 0x9c), (0x9d, 0x9e)]),
        "ddj-serato" => mixxx("ddj-sx"),
        "cdj" => with_decks34(mixxx("cdj-2000")),
        _ => vec![],
    }
}

pub fn midi_key(st: u8, d1: u8) -> u16 {
    let s = if st & 0xf0 == 0x80 { 0x90 | (st & 0x0f) } else { st };
    (s as u16) << 8 | d1 as u16
}
pub fn rel_ticks(v: u8) -> f64 { v as f64 - 64.0 } // jog wheels: centre 64, <64 backwards
pub fn rel_knob(v: u8) -> i32 { if v < 64 { v as i32 } else { v as i32 - 128 } } // browse: 1 = clockwise, 127 = anticlockwise

pub const ABS_ACTIONS: [&str; 10] = ["tempo", "fader", "trim", "hi", "mid", "low", "cfx", "xfader", "headMix", "master"];
pub const REL_ACTIONS: [&str; 5] = ["jogTop", "jogSide", "jogSearch", "browse", "zoom"];
pub fn is_led_action(a: &str) -> bool {
    ["play", "cue", "sync", "pfl", "loopIn", "loopOut", "reloop", "keylock", "quant", "mode:", "hot:", "loop:"].iter().any(|p| a.starts_with(p))
}

/// what a MIDI message asks for
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// a fader/knob position 0..1
    Abs { name: String, deck: i8, val: f64 },
    /// a jog wheel or knob turned by `ticks`
    Rel { name: String, deck: i8, ticks: f64 },
    /// a button
    Press { name: String, arg: String, deck: i8, on: bool },
}

#[derive(Clone, Debug)]
pub struct LearnTarget { pub action: String, pub deck: i8, got_touch: Option<Entry>, got_turn: Option<Entry> }
impl LearnTarget { pub fn new(action: &str, deck: i8) -> Self { LearnTarget { action: action.into(), deck, got_touch: None, got_turn: None } } }

pub struct Engine {
    pub profile: &'static str,
    pub table: HashMap<u16, Entry>,
    lsb_table: HashMap<u16, Entry>,
    msb: HashMap<u16, u8>,
    pub led_cache: HashMap<u16, u8>,
    pub led_entries: Vec<Entry>,
    pub learned: Vec<Entry>,
    /// Some = learn mode; the inner Some = a control picked on screen, waiting for the hardware
    pub learning: Option<Option<LearnTarget>>,
    /// messages waiting to go out to the controller
    pub out: Vec<Vec<u8>>,
    pub log: Vec<Vec<u8>>,
    pub hint: Option<String>,
    /// learned mappings changed (the app saves them)
    pub learned_changed: bool,
}

impl Default for Engine { fn default() -> Self { Engine::new() } }

impl Engine {
    pub fn new() -> Engine {
        Engine { profile: "learn", table: HashMap::new(), lsb_table: HashMap::new(), msb: HashMap::new(), led_cache: HashMap::new(), led_entries: vec![], learned: vec![], learning: None, out: vec![], log: vec![], hint: None, learned_changed: false }
    }
    pub fn use_profile(&mut self, id: &str, learned: Vec<Entry>) {
        let ps = profiles();
        let p = ps.iter().find(|p| p.id == id).unwrap_or_else(|| ps.iter().find(|p| p.id == "learn").unwrap());
        self.profile = p.id;
        self.learned = learned;
        self.table.clear(); self.lsb_table.clear(); self.msb.clear(); self.led_cache.clear();
        // learned entries win; earlier entries in a profile win over later ones
        for e in self.learned.iter().cloned().chain(profile_map(p.id)) {
            let k = midi_key(e.0, e.1);
            if e.4 != 0 { self.lsb_table.entry(midi_key(e.0, e.4)).or_insert_with(|| e.clone()); }
            self.table.entry(k).or_insert(e);
        }
        self.led_entries = if !p.leds && self.learned.is_empty() { vec![] } else {
            let mut v: Vec<Entry> = self.table.values().filter(|e| e.0 & 0xf0 == 0x90 && is_led_action(&e.2)).cloned().collect();
            v.sort_by_key(|e| (e.0, e.1));
            v
        };
    }
    pub fn profile_info(&self) -> Profile { profiles().into_iter().find(|p| p.id == self.profile).unwrap() }

    pub fn send(&mut self, bytes: Vec<u8>) {
        self.log.push(bytes.clone());
        if self.log.len() > 500 { self.log.drain(..250); }
        self.out.push(bytes);
    }
    /// set an LED (Pioneer hardware lights a button when it receives its own note back)
    pub fn led(&mut self, st: u8, note: u8, v: u8) {
        let k = (st as u16) << 8 | note as u16;
        if self.led_cache.get(&k) == Some(&v) { return; }
        self.led_cache.insert(k, v);
        self.send(vec![st, note, v]);
    }

    /// an incoming message -> what it asks for (None while learning or for unmapped messages)
    pub fn input(&mut self, data: &[u8]) -> Option<Action> {
        if data.len() < 2 { return None; }
        let (st, d1, d2) = (data[0], data[1], *data.get(2).unwrap_or(&0));
        let ty = st & 0xf0;
        if ty != 0x90 && ty != 0x80 && ty != 0xb0 { return None; }
        if self.learning.is_some() { self.learn_capture(st, d1, d2); return None; }
        let k = midi_key(st, d1);
        if let Some(e) = self.table.get(&k).cloned() { return self.run(&e, ty, d2, false); }
        if let Some(l) = self.lsb_table.get(&k).cloned() { if ty == 0xb0 { return self.run(&l, ty, d2, true); } }
        None
    }
    fn run(&mut self, e: &Entry, ty: u8, v: u8, is_lsb: bool) -> Option<Action> {
        let (name, arg) = match e.2.find(':') { Some(i) => (&e.2[..i], &e.2[i + 1..]), None => (e.2.as_str(), "") };
        if ty == 0xb0 && ABS_ACTIONS.contains(&name) {
            let k = midi_key(e.0, e.1);
            let val = if e.4 == 0 { v as f64 / 127.0 }
                else if !is_lsb { self.msb.insert(k, v); v as f64 / 127.0 } // apply the MSB straight away, refine on the LSB
                else { (((*self.msb.get(&k).unwrap_or(&0) as u32) << 7 | v as u32) as f64) / 16383.0 };
            return Some(Action::Abs { name: name.into(), deck: e.3, val });
        }
        if REL_ACTIONS.contains(&name) {
            if ty != 0xb0 { return None; }
            let ticks = if name.starts_with("jog") { rel_ticks(v) } else { rel_knob(v) as f64 };
            return Some(Action::Rel { name: name.into(), deck: e.3, ticks });
        }
        let on = if ty == 0x90 { v > 0 } else if ty == 0x80 { false } else { v > 63 };
        Some(Action::Press { name: name.into(), arg: arg.into(), deck: e.3, on })
    }

    /// LEDs: `state(action name, arg, deck)` says whether that light should be on
    pub fn update_leds(&mut self, mut state: impl FnMut(&str, &str, usize) -> Option<bool>, vu: [f32; 2]) {
        for e in self.led_entries.clone() {
            if e.3 < 0 { continue; }
            let (name, arg) = match e.2.find(':') { Some(i) => (&e.2[..i], &e.2[i + 1..]), None => (e.2.as_str(), "") };
            if let Some(on) = state(name, arg, e.3 as usize) { self.led(e.0, e.1, if on { 0x7f } else { 0 }); }
        }
        if self.profile_info().vu { for i in 0..2 { self.led(0xb0 + i as u8, 0x02, (vu[i] * 127.0).round().clamp(0.0, 127.0) as u8); } }
    }

    // ---- MIDI learn: pick a control on screen, then move or press it on the controller ----
    pub fn start_learn(&mut self) { self.learning = Some(None); }
    pub fn stop_learn(&mut self) { self.learning = None; }
    pub fn learn_pick(&mut self, action: &str, deck: i8) -> String {
        self.learning = Some(Some(LearnTarget::new(action, deck)));
        let who = format!("{}{}", if deck >= 0 { format!("Deck {} ", deck + 1) } else { String::new() }, learn_label(action));
        if action == "jog" { format!("{who}: touch the top of the platter, then turn it.") } else { format!("{who}: now move or press it on the controller.") }
    }
    /// learned entries changed: returns the hint to show; the caller saves `learned`
    fn learn_capture(&mut self, st: u8, d1: u8, d2: u8) -> Option<String> {
        let mut t = match self.learning.take() { Some(Some(t)) => t, other => { self.learning = other; return None; } };
        let ty = st & 0xf0;
        let s = if ty == 0x80 { 0x90 | (st & 0x0f) } else { st };
        if ty == 0x80 || (ty == 0x90 && d2 == 0) { self.learning = Some(Some(t)); return None; } // releases
        let entries: Vec<Entry>;
        if t.action == "jog" {
            if ty == 0x90 { t.got_touch = Some(Entry(s, d1, "touch".into(), t.deck, 0)); } else { t.got_turn = Some(Entry(s, d1, "jogTop".into(), t.deck, 0)); }
            match (&t.got_touch, &t.got_turn) {
                (Some(a), Some(b)) => entries = vec![a.clone(), b.clone()],
                (Some(_), None) => { self.hint = Some("Got the touch. Now turn the platter.".into()); self.learning = Some(Some(t)); return None; }
                _ => { self.hint = Some("Got the turn. Now touch the top of the platter.".into()); self.learning = Some(Some(t)); return None; }
            }
        } else if ABS_ACTIONS.contains(&t.action.as_str()) {
            if ty != 0xb0 { self.hint = Some("That sent a button press. Move a knob or fader instead.".into()); self.learning = Some(Some(t)); return None; }
            // Pioneer-style 14-bit pairs: the fine part arrives on CC + 32
            entries = vec![Entry(s, d1, t.action.clone(), t.deck, if d1 < 0x20 { d1 + 0x20 } else { 0 })];
        } else if t.action == "browse" {
            if ty != 0xb0 { self.hint = Some("Turn the knob rather than pressing it.".into()); self.learning = Some(Some(t)); return None; }
            entries = vec![Entry(s, d1, "browse".into(), -1, 0)];
        } else {
            entries = vec![Entry(s, d1, t.action.clone(), t.deck, 0)];
        }
        let keys: Vec<u16> = entries.iter().map(|e| midi_key(e.0, e.1)).collect();
        let targets: Vec<(String, i8)> = entries.iter().map(|e| (e.2.clone(), e.3)).collect();
        // one control does one thing: drop older learns on the same message or for the same control
        self.learned.retain(|e| !keys.contains(&midi_key(e.0, e.1)) && !targets.contains(&(e.2.clone(), e.3)));
        self.learned.extend(entries.iter().cloned());
        let p = self.profile;
        let learned = self.learned.clone();
        self.use_profile(p, learned);
        self.learning = Some(None);
        self.learned_changed = true;
        let h = entries.iter().map(|e| format!("{:02X} {:02X}", e.0, e.1)).collect::<Vec<_>>().join(" + ");
        self.hint = Some(format!("Mapped {h}. Click the next control, or Done."));
        None
    }
}

// learn feedback, read and cleared by the app
impl Engine {
    pub fn take_hint(&mut self) -> Option<String> { self.hint.take() }
}

pub fn learn_label(a: &str) -> String {
    let l = match a {
        "play" => "Play", "cue" => "Cue", "sync" => "Sync", "loopIn" => "Loop in", "loopOut" => "Loop out", "reloop" => "Reloop/exit",
        "tempo" => "Tempo fader", "jog" => "Jog wheel", "pfl" => "Headphone cue", "fader" => "Channel fader", "trim" => "Trim", "hi" => "EQ high",
        "mid" => "EQ mid", "low" => "EQ low", "cfx" => "Filter (CFX)", "xfader" => "Crossfader", "headMix" => "Cue/master mix", "master" => "Master level",
        "browse" => "Browse knob", "browsePress" => "Browse knob press", "load" => "Load", "keylock" => "Master tempo", "quant" => "Quantize", "jogSide" => "Jog side (nudge)",
        _ => "",
    };
    if !l.is_empty() { return l.into(); }
    if let Some(p) = a.strip_prefix("hot:") { return format!("Pad {}", p.parse::<usize>().unwrap_or(0) + 1); }
    a.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_names_pick_profiles() {
        let names = [("DDJ-FLX4", "ddj-flx4"), ("DDJ-FLX4 MIDI 1", "ddj-flx4"), ("DDJ-400", "ddj-400"), ("DDJ-200", "ddj-200"),
            ("PIONEER DDJ-SB3", "ddj-sb3"), ("DDJ-SB2", "ddj-sb2"), ("PIONEER DDJ-SB", "ddj-sb"), ("DDJ-SX2", "ddj-sx2"), ("DDJ-SX3", "ddj-sx2"),
            ("PIONEER DDJ-SX", "ddj-sx"), ("XDJ-RX", "xdj-rx"), ("CDJ-2000NXS2", "cdj-2000"), ("CDJ-850", "cdj-850"), ("CDJ-350", "cdj-350"),
            ("DDJ-FLX6", "ddj-rekordbox"), ("DDJ-FLX10", "ddj-rekordbox"), ("DDJ-FLX2", "ddj-rekordbox"), ("DDJ-1000", "ddj-rekordbox"),
            ("DDJ-800", "ddj-rekordbox"), ("DDJ-GRV6", "ddj-rekordbox"), ("XDJ-RX3", "ddj-rekordbox"), ("XDJ-XZ", "ddj-rekordbox"), ("XDJ-AZ", "ddj-rekordbox"),
            ("OPUS-QUAD", "ddj-rekordbox"), ("DDJ-REV7", "ddj-serato"), ("DDJ-SR2", "ddj-serato"), ("DDJ-1000SRT", "ddj-serato"), ("DDJ-SZ2", "ddj-serato"),
            ("CDJ-3000", "cdj"), ("XDJ-1000MK2", "cdj"), ("nanoKONTROL2", "learn"),
            // ALSA names on Linux carry a client prefix
            ("DDJ-FLX4:DDJ-FLX4 MIDI 1 24:0", "ddj-flx4")];
        for (n, want) in names { assert_eq!(profile_for(n), want, "{n}"); }
    }
    #[test]
    fn flx4_messages() {
        let mut e = Engine::new();
        e.use_profile("ddj-flx4", vec![]);
        assert_eq!(e.input(&[0x90, 0x0b, 0x7f]), Some(Action::Press { name: "play".into(), arg: "".into(), deck: 0, on: true }));
        assert_eq!(e.input(&[0x80, 0x0b, 0x00]), Some(Action::Press { name: "play".into(), arg: "".into(), deck: 0, on: false }));
        // 14-bit fader: MSB then LSB
        assert_eq!(e.input(&[0xb1, 0x13, 0x7f]), Some(Action::Abs { name: "fader".into(), deck: 1, val: 1.0 }));
        assert_eq!(e.input(&[0xb1, 0x33, 0x7f]), Some(Action::Abs { name: "fader".into(), deck: 1, val: 1.0 }));
        assert_eq!(e.input(&[0xb0, 0x00, 0x40]), Some(Action::Abs { name: "tempo".into(), deck: 0, val: 64.0 / 127.0 }));
        assert_eq!(e.input(&[0xb0, 0x20, 0x00]), Some(Action::Abs { name: "tempo".into(), deck: 0, val: (64.0 * 128.0) / 16383.0 }));
        assert_eq!(e.input(&[0xb6, 0x40, 0x7f]), Some(Action::Rel { name: "browse".into(), deck: -1, ticks: -1.0 }));
        assert_eq!(e.input(&[0xb0, 0x22, 0x41]), Some(Action::Rel { name: "jogTop".into(), deck: 0, ticks: 1.0 }));
        assert_eq!(e.input(&[0x97, 0x02, 0x7f]), Some(Action::Press { name: "hot".into(), arg: "2".into(), deck: 0, on: true }));
        assert_eq!(e.input(&[0x99, 0x64, 0x7f]), Some(Action::Press { name: "loop".into(), arg: "4".into(), deck: 1, on: true }));
        assert_eq!(e.input(&[0x96, 0x47, 0x7f]), Some(Action::Press { name: "load".into(), arg: "".into(), deck: 1, on: true }));
        assert!(!e.led_entries.is_empty());
    }
    #[test]
    fn every_profile_maps_play_and_a_fader() {
        for p in profiles() {
            if p.id == "learn" { continue; }
            let m = profile_map(p.id);
            assert!(m.iter().any(|e| e.2 == "play"), "{} play", p.id);
            assert!(m.iter().any(|e| e.2 == "fader" || e.2 == "tempo"), "{} fader/tempo", p.id);
        }
    }
    #[test]
    fn learn_maps_and_wins() {
        let mut e = Engine::new();
        e.use_profile("learn", vec![]);
        e.start_learn();
        e.learn_pick("fader", 0);
        e.input(&[0xb3, 0x05, 20]);
        e.learn_pick("play", 1);
        e.input(&[0x9f, 0x10, 127]);
        e.learn_pick("jog", 0);
        e.input(&[0x9f, 0x11, 127]);
        e.input(&[0xb3, 0x30, 70]);
        e.stop_learn();
        let l: Vec<String> = e.learned.iter().map(|x| format!("{},{},{},{}", x.0, x.1, x.2, x.3)).collect();
        assert_eq!(l, ["179,5,fader,0", "159,16,play,1", "159,17,touch,0", "179,48,jogTop,0"]);
        assert_eq!(e.input(&[0x9f, 0x10, 127]), Some(Action::Press { name: "play".into(), arg: "".into(), deck: 1, on: true }));
        assert!(e.learned_changed);
    }
}
