//! What Cratebox keeps between runs, as JSON files in ~/.local/share/cratebox (or $XDG_DATA_HOME):
//! settings, the analysis cache, cue points and playlists. Writes go to a temp file then rename.
//! The once-a-second saves serialise and write on a writer thread of their own (a big library's
//! analysis.json takes tens of ms: a dropped frame mid-set otherwise); `flush` waits for it, then
//! writes on the calling thread (quitting, shutting down, tests).

use crate::library::{CacheMap, Cues};
use crate::midi::Entry;
use crate::playlists::Playlist;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub folders: Vec<PathBuf>,
    pub devices: Vec<PathBuf>,
    pub bpm_range: usize,
    /// the custom BPM range (bpm_range == analysis::BPM_CUSTOM)
    pub bpm_lo: f64,
    pub bpm_hi: f64,
    /// deck area: "auto" (screens while a controller is connected), "screens" or "controls"
    pub deck_view: String,
    pub wave_style: String,
    pub spin: f64,
    /// master tempo per deck (a list, so older two-deck settings files still load)
    pub mt: Vec<bool>,
    /// 4-deck mode
    pub four: bool,
    /// with `four`: 3 decks only (1 and 3 share the left side, deck 2 has the right; no deck 4)
    pub three: bool,
    /// WAVE mode: just the waveforms (and the library)
    pub wave: bool,
    /// the BEAT FX unit: effect, LEVEL/DEPTH, channel (0-3, 4 = master) and the BEAT picked for each effect
    pub bfx_fx: String,
    pub bfx_depth: f64,
    pub bfx_ch: usize,
    pub bfx_beats: HashMap<String, f64>,
    /// where REC saves (empty: ~/Music/Cratebox Recordings)
    pub rec_dir: String,
    /// HEAT: curves pasted from TrueCurve, the curve picked last, HOLD temperature, PUFF full-warble dip
    /// (0 = Auto), the Puffco used last (its Bluetooth address) and each device's learned heater model
    pub heat_curves: HashMap<String, crate::heat::CurveDef>,
    pub heat_last: String,
    pub heat_hold_f: f64,
    pub heat_puff_full: f64,
    pub heat_device: String,
    /// GLOW: the Puffco's lantern as a slow rainbow that follows the music; what the FX LEVEL/DEPTH knob
    /// also sets: "intensity", "speed", "sensitivity" or "off"
    pub heat_glow: bool,
    pub heat_glow_knob: String,
    pub heat_models: HashMap<String, crate::heat::HeaterModel>,
    pub zoom: f64,
    /// the library's height and its playlist column's width when dragged (0 = automatic)
    pub lib_h: f32,
    pub tree_w: f32,
    pub browse: bool,
    /// loading a track closes BROWSE (back to the decks)
    pub browse_close_on_load: bool,
    /// nudges the scrolling waveforms against the sound, ms (+ = drawn later)
    pub wave_offset_ms: f64,
    /// header shows frames per second and missed screen refreshes (for chasing waveform stutter)
    pub show_fps: bool,
    /// colour theme (widgets::THEMES)
    pub theme: String,
    /// player view with the bottom library closed (the pop-up browser stands in for it)
    pub lib_hidden: bool,
    pub out_device: String,
    /// "auto" (4-channel device -> quad, else master only), "quad", "split", "device", "off"
    pub hp_mode: String,
    pub hp_device: String,
    pub midi_in: String,
    pub profile_for: HashMap<String, String>,
    pub learned: HashMap<String, Vec<Entry>>,
    /// where the music folder lives, for rekordbox XML export paths (normally the real path here)
    pub base_path: String,
    pub pl_open: Vec<String>,
    /// tracks that crashed or failed the analysis: not retried automatically
    pub bad: Vec<String>,
    pub buffer: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            folders: vec![], devices: vec![], bpm_range: 3, bpm_lo: 70.0, bpm_hi: 180.0, deck_view: "auto".into(), wave_style: "rgb".into(), spin: 2.5, mt: vec![true; 4], four: false, three: false, wave: false,
            bfx_fx: "ECHO".into(), bfx_depth: 0.55, bfx_ch: 0, bfx_beats: HashMap::new(), rec_dir: String::new(),
            heat_curves: HashMap::new(), heat_last: crate::heat::DEFAULT_NAME.into(), heat_hold_f: 500.0, heat_puff_full: 0.0, heat_device: String::new(), heat_glow: false, heat_glow_knob: "intensity".into(), heat_models: HashMap::new(), zoom: 8.0, lib_h: 0.0, tree_w: 0.0, browse: false, browse_close_on_load: true, wave_offset_ms: 0.0, show_fps: false, theme: "Night".into(), lib_hidden: false,
            out_device: String::new(), hp_mode: "auto".into(), hp_device: String::new(), midi_in: String::new(), profile_for: HashMap::new(),
            learned: HashMap::new(), base_path: String::new(), pl_open: vec![], bad: vec![], buffer: 256,
        }
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CRATEBOX_DATA") { return PathBuf::from(d); }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("cratebox")
}

pub fn load<T: DeserializeOwned + Default>(name: &str) -> T {
    let p = data_dir().join(name);
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
            eprintln!("cratebox: {} is damaged ({e}); keeping a copy and starting fresh", p.display());
            let _ = std::fs::copy(&p, p.with_extension("json.bad"));
            T::default()
        }),
        Err(_) => T::default(),
    }
}

pub fn save<T: Serialize>(name: &str, v: &T) -> Result<(), String> { write_file(name, to_json(v)?) }
fn to_json<T: Serialize>(v: &T) -> Result<Vec<u8>, String> { serde_json::to_vec(v).map_err(|e| e.to_string()) }
fn write_file(name: &str, data: Vec<u8>) -> Result<(), String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(name);
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

type WriteJob = Box<dyn FnOnce() + Send>;

/// everything persisted, with dirty flags so the app writes only what changed (at most once a second)
#[derive(Default)]
pub struct Store {
    writer: Option<std::sync::mpsc::Sender<WriteJob>>,
    /// files the writer thread couldn't save (marked dirty again on the next flush_bg) and why
    failed: std::sync::Arc<std::sync::Mutex<Vec<(&'static str, String)>>>,
    pub settings: Settings,
    pub cache: CacheMap,
    pub cues: HashMap<String, Cues>,
    pub playlists: Vec<Playlist>,
    pub dirty_settings: bool,
    pub dirty_cache: bool,
    pub dirty_cues: bool,
    pub dirty_playlists: bool,
}

impl Store {
    pub fn open() -> Store {
        Store { settings: load("settings.json"), cache: load("analysis.json"), cues: load("cues.json"), playlists: load("playlists.json"), ..Default::default() }
    }
    fn writer(&mut self) -> &std::sync::mpsc::Sender<WriteJob> {
        self.writer.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<WriteJob>();
            let _ = std::thread::Builder::new().name("cratebox-save".into()).spawn(move || { for job in rx { job(); } });
            tx
        })
    }
    /// save what changed on the writer thread; Some(error) if an earlier background save failed
    pub fn flush_bg(&mut self) -> Option<String> {
        let failed: Vec<(&'static str, String)> = self.failed.lock().map(|mut f| std::mem::take(&mut *f)).unwrap_or_default();
        for (name, _) in &failed {
            match *name { "settings.json" => self.dirty_settings = true, "analysis.json" => self.dirty_cache = true, "cues.json" => self.dirty_cues = true, _ => self.dirty_playlists = true }
        }
        let mut jobs: Vec<WriteJob> = vec![];
        let failed_to = self.failed.clone();
        let job = |name: &'static str, data: Result<Vec<u8>, String>| -> WriteJob {
            let f = failed_to.clone();
            Box::new(move || { if let Err(e) = data.and_then(|d| write_file(name, d)) { if let Ok(mut v) = f.lock() { v.push((name, e)); } } })
        };
        // serialising is cheap next to writing; doing it here keeps the data consistent without cloning it
        if std::mem::take(&mut self.dirty_settings) { jobs.push(job("settings.json", to_json(&self.settings))); }
        if std::mem::take(&mut self.dirty_cache) { jobs.push(job("analysis.json", to_json(&self.cache))); }
        if std::mem::take(&mut self.dirty_cues) { jobs.push(job("cues.json", to_json(&self.cues))); }
        if std::mem::take(&mut self.dirty_playlists) { jobs.push(job("playlists.json", to_json(&self.playlists))); }
        for j in jobs { let _ = self.writer().send(j); }
        failed.into_iter().next().map(|(n, e)| format!("{n}: {e}"))
    }
    /// wait until the writer thread has saved everything handed to it
    fn drain(&mut self) {
        let Some(w) = &self.writer else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        if w.send(Box::new(move || { let _ = tx.send(()); })).is_ok() { let _ = rx.recv_timeout(std::time::Duration::from_secs(10)); }
    }
    pub fn flush(&mut self) -> Result<(), String> {
        self.drain();
        let mut err = Ok(());
        let mut run = |dirty: &mut bool, r: Result<(), String>| { if r.is_ok() { *dirty = false; } else if err.is_ok() { err = r; } };
        if self.dirty_settings { let r = save("settings.json", &self.settings); run(&mut self.dirty_settings, r); }
        if self.dirty_cache { let r = save("analysis.json", &self.cache); run(&mut self.dirty_cache, r); }
        if self.dirty_cues { let r = save("cues.json", &self.cues); run(&mut self.dirty_cues, r); }
        if self.dirty_playlists { let r = save("playlists.json", &self.playlists); run(&mut self.dirty_playlists, r); }
        err
    }
}

pub fn exists(p: &Path) -> bool { p.exists() }
