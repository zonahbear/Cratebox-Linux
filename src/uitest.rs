//! Scripted UI tests: CRATEBOX_UITEST=script.json feeds real pointer/keyboard events into the window
//! (clicks, double-clicks, drags onto named on-screen targets), waits for the app to get somewhere,
//! records snapshots of its state, writes them to CRATEBOX_UITEST_OUT and quits.

use crate::app::App;
use eframe::egui::{self, pos2, Event, Key, Modifiers, PointerButton, Pos2, Rect};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "do", rename_all = "lowercase")]
pub enum Step {
    Wait { until: String, #[serde(default = "t30")] timeout: f64 },
    Frames { n: u32 },
    Click { target: String, #[serde(default)] fx: Option<f32>, #[serde(default)] fy: Option<f32>, #[serde(default)] double: bool, #[serde(default)] right: bool, #[serde(default)] ctrl: bool, #[serde(default)] shift: bool },
    Drag { from: String, #[serde(default)] to: Option<String>, #[serde(default)] dx: f32, #[serde(default)] dy: f32, #[serde(default)] fx: Option<f32>, #[serde(default)] fy: Option<f32>, #[serde(default = "steps")] steps: u32 },
    Key { key: String, #[serde(default)] shift: bool, #[serde(default)] ctrl: bool },
    Type { text: String },
    Dump { name: String },
}
fn t30() -> f64 { 30.0 }
fn steps() -> u32 { 12 }

pub struct UiTest {
    steps: VecDeque<Step>,
    queue: VecDeque<Vec<Event>>,
    started: Option<Instant>,
    frames_left: u32,
    out: PathBuf,
    pub snaps: serde_json::Map<String, serde_json::Value>,
    pub log: Vec<String>,
    pub done: bool,
    pos: Pos2,
}

impl UiTest {
    pub fn from_env() -> Option<UiTest> {
        let p = std::env::var("CRATEBOX_UITEST").ok()?;
        let steps: Vec<Step> = serde_json::from_slice(&std::fs::read(&p).ok()?).map_err(|e| eprintln!("uitest: bad script: {e}")).ok()?;
        let out = PathBuf::from(std::env::var("CRATEBOX_UITEST_OUT").unwrap_or_else(|_| "uitest-out.json".into()));
        Some(UiTest { steps: steps.into(), queue: VecDeque::new(), started: None, frames_left: 0, out, snaps: Default::default(), log: vec![], done: false, pos: pos2(5.0, 5.0) })
    }

    /// called before each frame: feed this frame's events
    pub fn input(&mut self, raw: &mut egui::RawInput, app: &App, rects: &HashMap<String, Rect>) {
        if let Some(evs) = self.queue.pop_front() {
            for e in &evs { if let Event::PointerMoved(p) = e { self.pos = *p; } }
            raw.events.extend(evs);
            return;
        }
        if self.frames_left > 0 { self.frames_left -= 1; return; }
        let Some(step) = self.steps.front().cloned() else { if !self.done { self.finish(app); } return };
        let at = |t: &str, fx: Option<f32>, fy: Option<f32>| -> Option<Pos2> {
            let r = rects.get(t)?;
            Some(pos2(r.left() + r.width() * fx.unwrap_or(0.5), r.top() + r.height() * fy.unwrap_or(0.5)))
        };
        let btn = |p: Pos2, b: PointerButton, pressed: bool, m: Modifiers| Event::PointerButton { pos: p, button: b, pressed, modifiers: m };
        match &step {
            Step::Wait { until, timeout } => {
                let t0 = *self.started.get_or_insert_with(Instant::now);
                if check(app, until) { self.next(); }
                else if t0.elapsed().as_secs_f64() > *timeout { self.log.push(format!("TIMEOUT waiting for {until}")); self.next(); }
                return;
            }
            Step::Frames { n } => { self.frames_left = *n; self.next(); return; }
            Step::Click { target, fx, fy, double, right, ctrl, shift } => {
                let Some(p) = at(target, *fx, *fy) else { self.missing(target); return };
                let m = Modifiers { ctrl: *ctrl, command: *ctrl, shift: *shift, ..Default::default() };
                let b = if *right { PointerButton::Secondary } else { PointerButton::Primary };
                self.queue.push_back(vec![Event::PointerMoved(p)]);
                for _ in 0..if *double { 2 } else { 1 } {
                    self.queue.push_back(vec![btn(p, b, true, m)]);
                    self.queue.push_back(vec![btn(p, b, false, m)]);
                }
                self.queue.push_back(vec![]);
            }
            Step::Drag { from, to, dx, dy, fx, fy, steps } => {
                let Some(a) = at(from, *fx, *fy) else { self.missing(from); return };
                let b = match to { Some(t) => match at(t, None, None) { Some(b) => b, None => { self.missing(t); return } }, None => pos2(a.x + dx, a.y + dy) };
                let m = Modifiers::default();
                self.queue.push_back(vec![Event::PointerMoved(a)]);
                self.queue.push_back(vec![btn(a, PointerButton::Primary, true, m)]);
                for k in 1..=*steps {
                    let f = k as f32 / *steps as f32;
                    self.queue.push_back(vec![Event::PointerMoved(pos2(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f))]);
                }
                self.queue.push_back(vec![Event::PointerMoved(b)]);
                self.queue.push_back(vec![btn(b, PointerButton::Primary, false, m)]);
                self.queue.push_back(vec![]);
            }
            Step::Key { key, shift, ctrl } => {
                let Some(k) = Key::from_name(key) else { self.log.push(format!("unknown key {key}")); self.next(); return };
                let m = Modifiers { shift: *shift, ctrl: *ctrl, command: *ctrl, ..Default::default() };
                self.queue.push_back(vec![Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }]);
                self.queue.push_back(vec![Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: m }]);
            }
            Step::Type { text } => self.queue.push_back(vec![Event::Text(text.clone())]),
            Step::Dump { name } => { self.snaps.insert(name.clone(), snapshot(app)); }
        }
        self.next();
    }
    fn next(&mut self) { self.steps.pop_front(); self.started = None; }
    fn missing(&mut self, t: &str) { self.log.push(format!("no on-screen target \"{t}\"")); self.next(); }
    fn finish(&mut self, app: &App) {
        self.done = true;
        self.snaps.insert("final".into(), snapshot(app));
        let v = serde_json::json!({ "snaps": self.snaps, "log": self.log });
        let _ = std::fs::write(&self.out, serde_json::to_vec_pretty(&v).unwrap_or_default());
    }
}

fn check(app: &App, what: &str) -> bool {
    match what {
        "tracks_ready" => !app.tracks.is_empty() && app.tracks.iter().filter(|t| t.device.is_none()).all(|t| t.analysis_current()),
        "deck0_loaded" => app.decks[0].loaded(),
        "deck1_loaded" => app.decks[1].loaded(),
        _ => false,
    }
}

pub fn snapshot(app: &App) -> serde_json::Value {
    use serde_json::json;
    let deck = |i: usize| {
        let d = &app.decks[i];
        json!({ "loaded": d.loaded(), "title": d.track.as_ref().map(|t| t.title.clone()), "playing": d.playing, "now": d.now(), "cue": d.cue,
            "hot": d.hot.to_vec(), "loop_on": d.lp.on, "sync": d.sync, "pitch": d.pitch, "rate": d.rate(), "bpm": d.bpm(), "duration": d.duration,
            "mt": d.mt, "quant": d.quant, "pad_mode": format!("{:?}", d.pad_mode) })
    };
    json!({
        "decks": [deck(0), deck(1)],
        "mixer": { "fader": app.mixer.fader, "low": app.mixer.low, "hi": app.mixer.hi, "xfader": app.mixer.xfader, "pfl": app.mixer.pfl, "master": app.mixer.master },
        "list": { "sel": app.list.sel, "sel_ids": app.list.sel_ids.len(), "titles": app.list.view.iter().map(|t| t.title.clone()).collect::<Vec<_>>(), "query": app.list.query },
        "view": app.lists.view_name(&app.view),
        "playlists": app.lists.lists.iter().map(|p| json!({ "name": p.name, "folder": p.folder, "items": p.items.len(), "parent": p.parent })).collect::<Vec<_>>(),
        "settings_open": app.ui.settings_open,
        "browse": app.store.settings.browse,
        "zoom": app.store.settings.zoom,
        "status": app.status_text(),
    })
}
