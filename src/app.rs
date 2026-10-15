//! The app: decks, mixer, library, playlists, controller and background work, tied together.
//! Everything here runs on the UI thread except the jobs it spawns (decoding, analysis, scanning),
//! which report back through a channel that `poll` drains every frame.

use crate::analysis::{analyze, fold_bpm, mono_for, phrase_analysis, Opts, Phrase, BPM_RANGES};
use crate::audio_io::{guess_controller_output, list_outputs, AudioIo, OutDev};
use crate::deck::{build_overview, master_for, Deck, DeckAna, DeckEvent, JogKind, PadMode, MasterInfo};
use crate::decode::{decode, resample};
use crate::engine::{Cmd, Engine, Shared, Status};
use crate::library::{is_audio, make_track, read_id3, scan_dir, Cached, Cues, ListState, Track};
use crate::midi::{self, Action, Entry};
use crate::midi_io::MidiIo;
use crate::mixer::{MixerParams, Route};
use crate::player::Pcm;
use crate::playlists::{self, rb_cues_for, read_device_analysis, Device, Lists, RowKind, TreeRow, View};
use crate::store::Store;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub enum Job {
    Scanned { root: PathBuf, tracks: Vec<Track>, quiet: bool },
    Loaded { deck: usize, gen: u64, track: Track, l: Pcm, r: Pcm, sr: f64, ana: DeckAna, mono: (Arc<Vec<f32>>, f64) },
    LoadFailed { deck: usize, gen: u64, msg: String },
    Analysed { id: String, res: Result<AnaResult, String> },
    Rephrased { deck: usize, gen: u64, phrases: Vec<Phrase> },
    Mounted { root: PathBuf, res: Result<(Device, Vec<Track>), String>, remember: bool },
}

pub struct AnaResult { pub bpm: f64, pub first_beat: f64, pub key: i32, pub phrases: Vec<Phrase>, pub duration: f64, pub title: Option<String>, pub artist: Option<String> }

/// UI-only state the app logic also touches (dialogs, editing)
#[derive(Default)]
pub struct UiState {
    pub grid_open: [bool; 2],
    pub bpm_text: [String; 2],
    pub settings_open: bool,
    pub renaming: Option<(String, String, bool)>,
    pub confirm_delete: Option<String>,
    pub learn_pick: Option<(String, i8)>,
    pub dialog: Option<crate::dialog::FileDialog>,
    pub add_menu: bool,
    pub pending_scroll_tree: bool,
}

pub struct App {
    pub tx: Sender<Cmd>,
    pub shared: Shared,
    pub status_snap: Status,
    pub audio: AudioIo,
    trash_rx: Receiver<Pcm>,
    pub decks: [Deck; 2],
    pub mixer: MixerParams,
    pub route: Route,
    pub tracks: Vec<Track>,
    pub by_id: HashMap<String, usize>,
    pub list: ListState,
    pub lists: Lists,
    pub view: View,
    pub focus_tree: bool,
    pub tree_sel: usize,
    pub store: Store,
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    queue: VecDeque<String>,
    analysing: Option<String>,
    analysed_count: usize,
    deck_loads: Arc<AtomicUsize>,
    pub midi: midi::Engine,
    pub midi_io: MidiIo,
    last_keep: Instant,
    last_led: Instant,
    pub status: String,
    status_until: Instant,
    pub progress: String,
    pub outputs: Vec<OutDev>,
    pub ui: UiState,
    last_flush: Instant,
    pub repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    pub hp_note: String,
    pub scanning: usize,
}

pub fn range_of(i: usize) -> (f64, f64) { let r = BPM_RANGES[i.min(3)]; (r.1, r.2) }
pub fn now_date() -> String {
    // yyyy-mm-dd from the system clock (UTC), for naming imported folders
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl App {
    pub fn new() -> App {
        let store = Store::open();
        let s = &store.settings;
        let outputs = list_outputs();
        let want = if !s.out_device.is_empty() && outputs.iter().any(|d| d.name == s.out_device) { s.out_device.clone() }
            else { guess_controller_output(&outputs).unwrap_or_default() };
        let sr = AudioIo::probe_rate(&want) as f64;
        let (tx, rx) = channel();
        let (trash_tx, trash_rx) = channel();
        let shared: Shared = Arc::new(Mutex::new(Status::default()));
        let engine = Arc::new(Mutex::new(Engine::new(sr, rx, trash_tx, shared.clone())));
        let mut audio = AudioIo::new(engine, sr);
        let open_err = audio.open(&want, s.buffer).err();
        let (jobs_tx, jobs_rx) = channel();
        let mut decks = [Deck::new(0, tx.clone()), Deck::new(1, tx.clone())];
        for (i, d) in decks.iter_mut().enumerate() { d.mt = s.mt[i]; d.spin_secs = s.spin; d.sr = sr; }
        let mut lists = Lists::default();
        lists.lists = store.playlists.clone();
        lists.open = s.pl_open.iter().cloned().collect();
        let mut app = App {
            tx, shared, status_snap: Status::default(), audio, trash_rx, decks, mixer: MixerParams::default(), route: Route::Master,
            tracks: vec![], by_id: HashMap::new(), list: ListState::new(), lists, view: View::All, focus_tree: false, tree_sel: 0, store,
            jobs_tx, jobs_rx, queue: VecDeque::new(), analysing: None, analysed_count: 0, deck_loads: Arc::new(AtomicUsize::new(0)),
            midi: midi::Engine::new(), midi_io: MidiIo::new(), last_keep: Instant::now(), last_led: Instant::now(),
            status: String::new(), status_until: Instant::now(), progress: String::new(), outputs, ui: UiState::default(), last_flush: Instant::now(),
            repaint: None, hp_note: String::new(), scanning: 0,
        };
        app.midi.use_profile("learn", vec![]);
        let _ = app.tx.send(Cmd::Mixer(app.mixer));
        app.apply_headphones(false);
        match open_err {
            Some(e) => app.say(&format!("No sound output: {e}. Pick one in Settings → Output."), 12.0),
            None => app.say(&format!("Sound: {}", if app.audio.device.is_empty() { "system default".to_string() } else { app.audio.device.clone() }), 4.0),
        }
        for f in app.store.settings.folders.clone() { app.scan_folder(&f, true); }
        for d in app.store.settings.devices.clone() { if d.join("PIONEER").exists() || d.exists() { app.mount(&d, false); } }
        app.connect_midi();
        app.apply_view(false);
        app
    }

    pub fn say(&mut self, s: &str, secs: f32) {
        self.status = s.to_string();
        self.status_until = Instant::now() + Duration::from_secs_f32(if secs <= 0.0 { 3600.0 } else { secs });
    }
    pub fn status_text(&self) -> &str { if Instant::now() < self.status_until { &self.status } else { "" } }
    pub fn range(&self) -> (f64, f64) { range_of(self.store.settings.bpm_range) }


    // ======================= library =======================
    fn reindex(&mut self) { self.by_id = self.tracks.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect(); }
    pub fn scan_folder(&mut self, root: &Path, quiet: bool) {
        let root = root.to_path_buf();
        let tx = self.jobs_tx.clone();
        let wake = self.repaint.clone();
        self.scanning += 1;
        if !quiet { self.say(&format!("Scanning {}…", root.display()), 0.0); }
        std::thread::spawn(move || {
            let files = scan_dir(&root);
            let mut tracks = vec![];
            for p in files {
                if let Some(mut t) = make_track(&p, &root) {
                    if p.extension().map(|e| e.eq_ignore_ascii_case("mp3")).unwrap_or(false) {
                        if let Some((a, ti)) = read_id3(&p) { if !ti.is_empty() { t.title = ti; } if !a.is_empty() { t.artist = a; } }
                    }
                    tracks.push(t);
                }
            }
            let _ = tx.send(Job::Scanned { root, tracks, quiet });
            if let Some(w) = wake { w(); }
        });
    }
    pub fn add_music_folder(&mut self, root: &Path) {
        if !self.store.settings.folders.iter().any(|f| f == root) { self.store.settings.folders.push(root.to_path_buf()); self.store.dirty_settings = true; }
        self.scan_folder(root, false);
    }
    pub fn remove_music_folder(&mut self, root: &Path) {
        self.store.settings.folders.retain(|f| f != root);
        self.store.dirty_settings = true;
        let on_deck: Vec<String> = self.decks.iter().filter_map(|d| d.track.as_ref().map(|t| t.id.clone())).collect();
        self.tracks.retain(|t| t.device.is_some() || !t.path.starts_with(root) || on_deck.contains(&t.id));
        self.reindex();
        self.apply_view(true);
    }
    fn add_tracks(&mut self, new: Vec<Track>, quiet: bool) {
        let before = self.tracks.len();
        let range = self.range();
        for mut t in new {
            if self.by_id.contains_key(&t.id) { continue; }
            if let Some(c) = self.store.cache.get(&t.id) { c.apply(&mut t, range); }
            let current = t.analysis_current();
            let bad = self.store.settings.bad.contains(&t.id);
            self.by_id.insert(t.id.clone(), self.tracks.len());
            if !current && !bad { self.queue.push_back(t.id.clone()); }
            if bad && t.bpm.is_none() { t.failed = true; }
            self.tracks.push(t);
        }
        let added = self.tracks.len() - before;
        self.lists.rematch(&self.tracks);
        if !quiet || added > 0 { self.say(&format!("Added {added} tracks"), 4.0); }
        self.apply_view(true);
    }
    /// a single file dropped onto the window
    pub fn add_loose_file(&mut self, p: &Path) -> Option<String> {
        if !is_audio(p) { return None; }
        let root = p.parent()?.to_path_buf();
        let t = make_track(p, &root)?;
        let id = t.id.clone();
        if !self.by_id.contains_key(&id) { self.add_tracks(vec![t], true); }
        Some(id)
    }
    pub fn track(&self, id: &str) -> Option<&Track> { self.by_id.get(id).map(|&i| &self.tracks[i]) }

    /// write a track's analysis back into the library and the cache
    pub fn save_track(&mut self, t: &Track) {
        if let Some(&i) = self.by_id.get(&t.id) {
            let keep = self.tracks[i].clone();
            let mut n = t.clone();
            n.path = keep.path; n.rel = keep.rel; n.name = keep.name; n.size = keep.size; n.mtime = keep.mtime; n.device = keep.device; n.anlz_path = keep.anlz_path;
            n.missing = false;
            self.tracks[i] = n;
        }
        self.store.cache.insert(t.id.clone(), Cached::of(t));
        self.store.dirty_cache = true;
    }
    pub fn view_tracks(&self) -> Vec<Track> {
        match &self.view {
            View::All => self.tracks.iter().filter(|t| t.device.is_none()).cloned().collect(),
            View::Pl(id) => self.lists.tracks(id, &self.by_id, &self.tracks),
            View::Dev(d) => self.tracks.iter().filter(|t| t.device.as_deref() == Some(d)).cloned().collect(),
            View::DevPl(d, n) => {
                let Some(dev) = self.lists.devices.iter().find(|x| &x.id == d) else { return vec![] };
                let Some(node) = dev.tree.iter().find(|x| x.id == *n) else { return vec![] };
                node.tracks.iter().filter_map(|r| dev.by_rb.get(r)).filter_map(|id| self.track(id)).cloned().collect()
            }
        }
    }
    pub fn apply_view(&mut self, keep_sel: bool) {
        let base = self.view_tracks();
        self.list.apply(base, keep_sel);
    }
    pub fn set_view(&mut self, v: View) {
        if matches!(v, View::Pl(_)) { self.list.sort = None; } // a playlist opens in its own order (so it can be rearranged)
        self.view = v;
        self.list.sel = 0; self.list.anchor = 0; self.list.sel_ids.clear();
        self.list.scroll_to_sel = true;
        self.apply_view(false);
    }
    pub fn is_own_playlist_view(&self) -> bool { matches!(self.view, View::Pl(_)) && self.list.sort.is_none() && self.list.query.is_empty() }
    pub fn set_bpm_range(&mut self, i: usize) {
        self.store.settings.bpm_range = i;
        self.store.dirty_settings = true;
        let r = self.range();
        for t in self.tracks.iter_mut() { if !t.grid_manual { if let Some(b) = t.bpm { t.bpm = Some(fold_bpm(b, r)); } } }
        self.apply_view(true);
        self.say(&format!("BPM range {}: half/double-time tracks now read in that range", BPM_RANGES[i].0), 5.0);
    }

    // ======================= loading + analysis =======================
    pub fn load_selected(&mut self, deck: usize) {
        let Some(t) = self.list.view.get(self.list.sel).cloned() else { return };
        self.load_track(deck, t);
    }
    pub fn load_id(&mut self, deck: usize, id: &str) { if let Some(t) = self.track(id).cloned() { self.load_track(deck, t); } }
    pub fn load_track(&mut self, i: usize, t: Track) {
        if t.missing { self.say(&format!("\"{}\" isn't in a connected folder or device right now", t.title), 5.0); return; }
        let t = self.track(&t.id).cloned().unwrap_or(t);
        let gen = match self.decks[i].begin_load(t.clone()) { Ok(g) => g, Err(e) => { self.say(&e, 4.0); return; } };
        self.say(&format!("Loading {} into deck {}…", t.title, i + 1), 0.0);
        let tx = self.jobs_tx.clone();
        let sr = self.audio.sr;
        let range = self.range();
        let loads = self.deck_loads.clone();
        let wake = self.repaint.clone();
        loads.fetch_add(1, Ordering::SeqCst);
        std::thread::Builder::new().name("cratebox-load".into()).spawn(move || {
            let res = (|| -> Result<Job, String> {
                let mut track = t;
                if track.device.is_some() && !track.rb_read { read_device_analysis(&mut track); }
                let d = decode(&track.path)?;
                if let (Some(ti), true) = (&d.title, track.artist.is_empty()) { track.title = ti.clone(); }
                if let (Some(a), true) = (&d.artist, track.artist.is_empty()) { track.artist = a.clone(); }
                let (mono, fs) = mono_for(&[&d.l, &d.r], d.sr, 11025.0);
                let (w, fsw) = mono_for(&[&d.l, &d.r], d.sr, 22050.0);
                let duration = d.duration();
                let (l, r) = if (d.sr as f64 - sr).abs() > 0.5 { (resample(&d.l, d.sr as f64, sr), resample(&d.r, d.sr as f64, sr)) } else { (d.l, d.r) };
                let cached = track.analysis_current() && track.bpm.unwrap_or(0.0) > 0.0 && track.first_beat.is_some();
                let grid = if track.grid_manual { track.bpm.zip(track.first_beat) } else { None };
                let a = analyze(&mono, fs, Some((&w, fsw)), range, Opts { wave_only: cached, grid, ..Default::default() });
                let (mut bpm, mut first, mut key, mut phrases) = (a.bpm, a.first_beat, a.key, a.phrases.clone());
                if cached || track.grid_manual { bpm = track.bpm.unwrap_or(bpm); first = track.first_beat.unwrap_or(first); }
                if cached { key = track.key.unwrap_or(-1); phrases = track.phrases.clone().unwrap_or_default(); }
                else if track.key_from.as_deref() == Some("rekordbox") && track.key.unwrap_or(-1) >= 0 { key = track.key.unwrap(); }
                let wave = a.wave.unwrap_or_default();
                let overview = build_overview(&wave);
                let ana = DeckAna { bpm, first_beat: first, key, phrases, wave: Arc::new(wave), overview: Arc::new(overview), duration: l.len() as f64 / sr };
                let _ = duration;
                Ok(Job::Loaded { deck: i, gen, track, l: Arc::new(l), r: Arc::new(r), sr, ana, mono: (Arc::new(mono), fs) })
            })();
            loads.fetch_sub(1, Ordering::SeqCst);
            let _ = tx.send(match res { Ok(j) => j, Err(msg) => Job::LoadFailed { deck: i, gen, msg } });
            if let Some(w) = wake { w(); }
        }).ok();
    }
    fn run_queue(&mut self) {
        if self.analysing.is_some() { return; }
        // deck loads go first
        if self.deck_loads.load(Ordering::SeqCst) > 0 { if !self.queue.is_empty() { self.progress = "Analysis waiting…".into(); } return; }
        while let Some(id) = self.queue.pop_front() {
            let Some(t) = self.track(&id).cloned() else { continue };
            if t.analysis_current() || t.device.is_some() { continue; }
            self.analysing = Some(id.clone());
            let total = self.analysed_count + self.queue.len() + 1;
            self.progress = format!("Analyzing {} of {}: {}", self.analysed_count + 1, total, t.title);
            let tx = self.jobs_tx.clone();
            let range = self.range();
            let wake = self.repaint.clone();
            std::thread::Builder::new().name("cratebox-analyse".into()).spawn(move || {
                let res = (|| -> Result<AnaResult, String> {
                    let d = decode(&t.path)?;
                    let (mono, fs) = mono_for(&[&d.l, &d.r], d.sr, 11025.0);
                    drop(d.l); drop(d.r);
                    let grid = if t.grid_manual { t.bpm.zip(t.first_beat) } else { None };
                    let a = analyze(&mono, fs, None, range, Opts { no_wave: true, grid, ..Default::default() });
                    Ok(AnaResult { bpm: a.bpm, first_beat: a.first_beat, key: a.key, phrases: a.phrases, duration: mono.len() as f64 / fs, title: d.title, artist: d.artist })
                })();
                let _ = tx.send(Job::Analysed { id, res });
                if let Some(w) = wake { w(); }
            }).ok();
            return;
        }
        if self.analysed_count > 0 || !self.progress.is_empty() {
            self.progress = format!("{} tracks", self.tracks.iter().filter(|t| t.device.is_none()).count());
        }
    }

    // ======================= devices + XML =======================
    pub fn mount(&mut self, root: &Path, remember: bool) {
        let root = root.to_path_buf();
        let tx = self.jobs_tx.clone();
        let saved = self.store.cache.clone();
        let range = self.range();
        let wake = self.repaint.clone();
        std::thread::spawn(move || {
            let res = playlists::mount_device(&root, &saved, range);
            let _ = tx.send(Job::Mounted { root, res, remember });
            if let Some(w) = wake { w(); }
        });
    }
    pub fn close_device(&mut self, id: &str) {
        if self.decks.iter().any(|d| d.track.as_ref().and_then(|t| t.device.as_deref()) == Some(id)) { self.say("Load something else on the decks first", 4.0); return; }
        let Some(d) = self.lists.devices.iter().find(|x| x.id == id).cloned() else { return };
        self.lists.devices.retain(|x| x.id != id);
        self.tracks.retain(|t| t.device.as_deref() != Some(id));
        self.reindex();
        self.store.settings.devices.retain(|p| p != &d.root);
        self.store.dirty_settings = true;
        if matches!(&self.view, View::Dev(x) | View::DevPl(x, _) if x == id) { self.set_view(View::All); } else { self.apply_view(true); }
        self.say(&format!("Closed {}", d.name), 3.0);
    }
    pub fn copy_device_playlist(&mut self, dev: &str, node: u32) {
        let Some(d) = self.lists.devices.iter().find(|x| x.id == dev).cloned() else { return };
        let Some(n) = d.tree.iter().find(|x| x.id == node).cloned() else { return };
        let tracks: Vec<Track> = n.tracks.iter().filter_map(|r| d.by_rb.get(r)).filter_map(|id| self.track(id).cloned()).collect();
        let id = self.lists.create(&n.name, false, None);
        let k = self.lists.add(&id, &tracks, None);
        self.say(&format!("Copied \"{}\" to your playlists ({k} tracks). It plays from {} while that's plugged in.", n.name, d.name), 8.0);
    }
    pub fn import_xml(&mut self, path: &Path) {
        let text = match std::fs::read_to_string(path) { Ok(t) => t, Err(e) => { self.say(&format!("Couldn't read that file: {e}"), 8.0); return; } };
        let date = now_date();
        match playlists::import_xml(&text, &mut self.tracks, &self.store.cues, &mut self.lists, &date) {
            Ok(r) => {
                for (id, c) in r.cue_sets { self.store.cues.insert(id, c); }
                self.store.dirty_cues = true;
                // save what rekordbox gave us; tracks not yet analysed still get length + phrases (keeping its grid and key)
                for t in self.tracks.clone() {
                    if t.grid_from.as_deref() == Some("rekordbox") || t.key_from.as_deref() == Some("rekordbox") {
                        self.store.cache.insert(t.id.clone(), Cached::of(&t));
                        if !t.analysis_current() && !self.queue.contains(&t.id) { self.queue.push_back(t.id.clone()); }
                    }
                }
                self.store.dirty_cache = true;
                self.apply_view(true);
                self.say(&format!("rekordbox XML: {} tracks ({} found in your folders, grids/cues/keys taken from rekordbox for {}), {} playlists",
                    r.tracks, r.matched, r.applied, r.playlists), 12.0);
            }
            Err(e) => self.say(&format!("Couldn't read that file: {e}"), 8.0),
        }
    }
    pub fn export_xml(&mut self, path: &Path) {
        let x = playlists::export_xml(&self.tracks, &self.store.cues, &self.lists, &self.store.settings.base_path);
        match std::fs::write(path, x) {
            Ok(_) => {
                let n = self.tracks.iter().filter(|t| t.device.is_none()).count();
                self.say(&format!("Exported {n} tracks and {} playlists to {}. In rekordbox: Preferences → Advanced → Imported Library = this file, and tick rekordbox xml under View → Layout.",
                    self.lists.lists.iter().filter(|p| !p.folder).count(), path.display()), 14.0);
            }
            Err(e) => self.say(&format!("Couldn't write {}: {e}", path.display()), 8.0),
        }
    }

    // ======================= playlists =======================
    pub fn selected_tracks(&self) -> Vec<Track> { self.list.selected() }
    pub fn add_to_playlist(&mut self, id: &str, tracks: &[Track]) {
        let n = self.lists.add(id, tracks, None);
        let name = self.lists.get(id).map(|p| p.name.clone()).unwrap_or_default();
        self.say(&format!("Added {n} track{} to {name}", if n == 1 { "" } else { "s" }), 3.0);
        if self.view == View::Pl(id.to_string()) { self.apply_view(true); }
    }
    pub fn remove_from_playlist(&mut self) {
        let View::Pl(id) = self.view.clone() else { return };
        let pos = self.list.selected_positions();
        if pos.is_empty() { return; }
        self.lists.remove_at(&id, &pos);
        self.list.sel_ids.clear();
        self.apply_view(true);
        let name = self.lists.get(&id).map(|p| p.name.clone()).unwrap_or_default();
        self.say(&format!("Removed {} from {name}", pos.len()), 3.0);
    }
    pub fn move_in_playlist(&mut self, positions: &[usize], to: usize) {
        let View::Pl(id) = self.view.clone() else { return };
        let new_pos = self.lists.move_items(&id, positions, to);
        self.apply_view(false);
        self.list.sel_ids = new_pos.iter().filter_map(|&p| self.list.view.get(p).map(|t| t.id.clone())).collect();
        if let Some(&p) = new_pos.first() { self.list.sel = p; }
    }
    pub fn delete_playlist(&mut self, id: &str) {
        let name = self.lists.get(id).map(|p| p.name.clone()).unwrap_or_default();
        let gone = self.lists.delete(id);
        if let View::Pl(v) = &self.view { if gone.contains(v) { self.set_view(View::All); } }
        self.say(&format!("Deleted {name}"), 3.0);
    }

    // ---- the tree, also driven by the FLX4's browse knob ----
    pub fn tree_rows(&self) -> Vec<TreeRow> { self.lists.tree_rows(self.tracks.iter().filter(|t| t.device.is_none()).count()) }
    pub fn open_row(&mut self, r: &TreeRow) {
        match r.kind {
            RowKind::All => self.set_view(View::All),
            RowKind::Pl => self.set_view(View::Pl(r.id.clone())),
            RowKind::Dev => { self.lists.open.insert(format!("dev:{}", r.dev)); self.set_view(View::Dev(r.dev.clone())); }
            RowKind::DevPl => self.set_view(View::DevPl(r.dev.clone(), r.rb)),
            RowKind::Folder | RowKind::DevFolder => self.lists.toggle_open(r),
            RowKind::Head(_) => {}
        }
    }
    pub fn tree_move(&mut self, n: i32) {
        let rows = self.tree_rows();
        let mut i = self.tree_sel.min(rows.len().saturating_sub(1)) as i64;
        for _ in 0..n.unsigned_abs() {
            let mut j = i + n.signum() as i64;
            while j >= 0 && (j as usize) < rows.len() && matches!(rows[j as usize].kind, RowKind::Head(_)) { j += n.signum() as i64; }
            if j < 0 || j as usize >= rows.len() { break; }
            i = j;
        }
        self.tree_sel = i as usize;
        let r = rows[self.tree_sel].clone();
        // show what's in it as you scroll, like rekordbox
        if matches!(r.kind, RowKind::All | RowKind::Pl | RowKind::DevPl) { self.open_row(&r); }
        self.ui.pending_scroll_tree = true;
    }
    pub fn tree_press(&mut self) {
        let rows = self.tree_rows();
        if !self.focus_tree {
            self.focus_tree = true;
            self.tree_sel = rows.iter().position(|r| playlists::row_is_view(r, &self.view)).unwrap_or(0);
            return;
        }
        let Some(r) = rows.get(self.tree_sel).cloned() else { return };
        match r.kind {
            RowKind::Folder | RowKind::DevFolder => { self.lists.toggle_open(&r); return; }
            RowKind::Dev if !self.lists.open.contains(&format!("dev:{}", r.dev)) => { self.open_row(&r); return; }
            _ => {}
        }
        if !playlists::row_is_view(&r, &self.view) { self.open_row(&r); }
        self.focus_tree = false;
    }

    // ======================= mixer + output =======================
    pub fn send_mixer(&mut self) { let _ = self.tx.send(Cmd::Mixer(self.mixer)); }
    pub fn apply_headphones(&mut self, announce: bool) {
        let ch = self.audio.channels;
        let mode = self.store.settings.hp_mode.clone();
        let mut route = Route::Master;
        let mut want_dev = None;
        let msg = match mode.as_str() {
            "split" => { route = Route::Split; "Split cue: mono master in the left channel, mono cue in the right".to_string() }
            "device" => {
                want_dev = Some(self.store.settings.hp_device.clone());
                format!("Master on {}; headphones on {}", self.out_name(), if self.store.settings.hp_device.is_empty() { "the default device" } else { &self.store.settings.hp_device })
            }
            "off" => "Headphone cue off".into(),
            "quad" if ch < 4 => format!("{} has only {ch} channels, so the controller's headphone jack (channels 3–4) can't be reached. Pick its hw: device in Settings → Output.", self.out_name()),
            _ if ch >= 4 => { route = Route::Quad; format!("Master on channels 1–2, headphones on 3–4 of {}", self.out_name()) }
            _ => format!("{} has {ch} channels: master only. Pick \"Another device\" for headphones, or Split with a splitter cable.", self.out_name()),
        };
        self.route = route;
        let _ = self.tx.send(Cmd::Route(route));
        let r = match &want_dev { Some(d) => self.audio.open_headphones(Some(if d.is_empty() { "default" } else { d.as_str() })), None => self.audio.open_headphones(None) };
        self.hp_note = msg.clone();
        if let Err(e) = r { self.say(&format!("Couldn't open the headphone device: {e}"), 8.0); return; }
        if announce { self.say(&msg, 10.0); }
    }
    pub fn out_name(&self) -> String { if self.audio.device.is_empty() { "the default output".into() } else { self.audio.device.clone() } }
    pub fn set_output(&mut self, name: &str) {
        let buf = self.store.settings.buffer;
        match self.audio.open(name, buf) {
            Ok(()) => { self.store.settings.out_device = name.to_string(); self.store.dirty_settings = true; self.apply_headphones(true); }
            Err(e) => self.say(&format!("Couldn't switch output: {e}"), 8.0),
        }
    }
    pub fn refresh_outputs(&mut self) { self.outputs = list_outputs(); }

    // ======================= controller =======================
    pub fn connect_midi(&mut self) {
        let ports = self.midi_io.ports.clone();
        let want = &self.store.settings.midi_in;
        let pick = ports.iter().find(|p| p == &want).or_else(|| ports.iter().find(|p| midi::pioneerish(p))).or_else(|| ports.first()).cloned();
        let Some(name) = pick else { self.midi_io.disconnect(); return };
        if self.midi_io.name == name { return; }
        if let Err(e) = self.midi_io.connect(&name) { self.say(&format!("Couldn't open {name}: {e}"), 6.0); return; }
        let forced = self.store.settings.profile_for.get(&name).cloned();
        let pid = forced.unwrap_or_else(|| midi::profile_for(&name).to_string());
        self.use_profile(&pid);
        let p = self.midi.profile_info();
        if let Some(init) = p.init { self.midi_io.send(init); }
        self.say(&format!("{name} connected · {}{}", p.name, if p.src == midi::Src::Tested { "" } else { " (see Settings → Controller)" }), 6.0);
    }
    pub fn use_profile(&mut self, id: &str) {
        let learned = self.store.settings.learned.get(id).cloned().unwrap_or_default();
        self.midi.use_profile(id, learned);
    }
    pub fn pick_profile(&mut self, id: &str) {
        self.use_profile(id);
        if !self.midi_io.name.is_empty() { self.store.settings.profile_for.insert(self.midi_io.name.clone(), id.to_string()); self.store.dirty_settings = true; }
    }
    pub fn pick_midi_in(&mut self, name: &str) {
        self.store.settings.midi_in = name.to_string();
        self.store.dirty_settings = true;
        self.midi_io.disconnect();
        self.connect_midi();
    }
    pub fn clear_learned(&mut self) {
        let p = self.midi.profile;
        self.store.settings.learned.remove(p);
        self.store.dirty_settings = true;
        self.use_profile(p);
        self.say("Cleared your learned controls", 3.0);
    }
    pub fn export_mapping(&mut self, path: &Path) {
        #[derive(serde::Serialize)]
        struct M<'a> { cratebox: u32, profile: &'a str, device: &'a str, learned: &'a [Entry] }
        let m = M { cratebox: 1, profile: self.midi.profile, device: &self.midi_io.name, learned: &self.midi.learned };
        match serde_json::to_string_pretty(&m).map_err(|e| e.to_string()).and_then(|s| std::fs::write(path, s).map_err(|e| e.to_string())) {
            Ok(_) => self.say(&format!("Saved the mapping to {}", path.display()), 6.0),
            Err(e) => self.say(&format!("Couldn't save the mapping: {e}"), 6.0),
        }
    }
    pub fn import_mapping(&mut self, path: &Path) {
        #[derive(serde::Deserialize)]
        struct M { profile: String, learned: Vec<Entry> }
        let r = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<M>(&b).map_err(|e| e.to_string()));
        match r {
            Ok(m) => {
                let pid = midi::profiles().into_iter().find(|p| p.id == m.profile).map(|p| p.id).unwrap_or(self.midi.profile);
                let n = m.learned.len();
                self.store.settings.learned.insert(pid.to_string(), m.learned);
                if !self.midi_io.name.is_empty() { self.store.settings.profile_for.insert(self.midi_io.name.clone(), pid.to_string()); }
                self.store.dirty_settings = true;
                self.use_profile(pid);
                self.say(&format!("Imported {n} mapped controls"), 5.0);
            }
            Err(e) => self.say(&format!("Couldn't import that mapping: {e}"), 8.0),
        }
    }
    pub fn learning(&self) -> bool { self.midi.learning.is_some() }
    /// in learn mode, clicking an on-screen control picks it for learning instead of using it
    pub fn learn_pick(&mut self, action: &str, deck: i8) {
        let hint = self.midi.learn_pick(action, deck);
        self.ui.learn_pick = Some((action.to_string(), deck));
        self.say(&hint, 0.0);
    }
    pub fn start_learn(&mut self) {
        if self.midi_io.name.is_empty() && self.midi.profile == "learn" { self.use_profile("learn"); }
        self.midi.start_learn();
        self.ui.settings_open = false;
        self.say("Learn: click a control on screen, then move or press it on the controller.", 0.0);
    }
    pub fn stop_learn(&mut self) { self.midi.stop_learn(); self.ui.learn_pick = None; self.say("", 0.1); }

    fn master(&self, i: usize) -> Option<MasterInfo> { master_for(&self.decks[i], &self.decks[1 - i]) }
    pub fn toggle_play(&mut self, i: usize) { let m = self.master(i); self.decks[i].toggle_play(m); }
    pub fn set_sync(&mut self, i: usize, on: bool) { let m = self.master(i); self.decks[i].set_sync(on, m); }

    pub fn midi_action(&mut self, a: Action) {
        match a {
            Action::Abs { name, deck, val } => {
                let d = deck.max(0) as usize;
                let v = val as f32;
                match name.as_str() {
                    "tempo" => { let p = val * 2.0 - 1.0; self.decks[d].set_pitch(p); } // top = slower, bottom = faster, like a CDJ
                    "fader" => { self.mixer.fader[d] = v; self.send_mixer(); }
                    "trim" => { self.mixer.trim[d] = v; self.send_mixer(); }
                    "hi" => { self.mixer.hi[d] = v; self.send_mixer(); }
                    "mid" => { self.mixer.mid[d] = v; self.send_mixer(); }
                    "low" => { self.mixer.low[d] = v; self.send_mixer(); }
                    "cfx" => { self.mixer.cfx[d] = v; self.send_mixer(); }
                    "xfader" => { self.mixer.xfader = v; self.send_mixer(); }
                    "headMix" => { self.mixer.head_mix = v; self.send_mixer(); }
                    "master" => { self.mixer.master = v; self.send_mixer(); }
                    _ => {}
                }
            }
            Action::Rel { name, deck, ticks } => {
                let d = deck.max(0) as usize;
                match name.as_str() {
                    "jogTop" => self.decks[d].jog_ticks(ticks, JogKind::Scratch),
                    "jogSide" => self.decks[d].jog_ticks(ticks, JogKind::Bend),
                    "jogSearch" => self.decks[d].jog_ticks(ticks, JogKind::Search),
                    "browse" => if self.focus_tree { self.tree_move(ticks as i32) } else { self.list.move_sel(ticks as i32) },
                    "zoom" => { let z = &mut self.store.settings.zoom; *z = (*z * if ticks > 0.0 { 0.8 } else { 1.25 }).clamp(2.0, 32.0); }
                    _ => {}
                }
            }
            Action::Press { name, arg, deck, on } => self.press(&name, &arg, deck, on),
        }
    }
    fn press(&mut self, name: &str, arg: &str, deck: i8, on: bool) {
        let i = deck.max(0) as usize;
        let num = || arg.parse::<f64>().unwrap_or(0.0);
        match name {
            "play" => if on { self.toggle_play(i) },
            "playCue" => if on { let c = self.decks[i].cue; self.decks[i].seek(c); let m = self.master(i); self.decks[i].set_playing(true, m); },
            "cue" => if on { self.decks[i].cue_down() } else { self.decks[i].cue_up() },
            "start" => if on { self.decks[i].set_playing(false, None); self.decks[i].seek(0.0); },
            "sync" => if on { let s = !self.decks[i].sync; self.set_sync(i, s); },
            "syncOn" => if on && !self.decks[i].sync { self.set_sync(i, true) },
            "range" => if on { self.decks[i].cycle_range() },
            "touch" => self.decks[i].jog_touch(on, false),
            "loopIn" => { // hold for half a second: 4-beat loop
                if on { self.decks[i].loop_in_down = Some(Instant::now()); self.decks[i].loop_in(); }
                else if self.decks[i].loop_in_down.take().is_some_and(|t| t.elapsed() > Duration::from_millis(500)) { self.decks[i].beat_loop(4.0); }
            }
            "loopOut" => if on { self.decks[i].loop_out() },
            "reloop" => if on { self.decks[i].reloop_exit() },
            "reloopStop" => if on { self.decks[i].reloop_exit(); if self.decks[i].lp.on { self.decks[i].set_playing(false, None); } },
            "loopHalf" => if on { self.decks[i].loop_scale(0.5) },
            "loopDouble" => if on { self.decks[i].loop_scale(2.0) },
            "callPrev" => if on { self.decks[i].call_cue(-1) },
            "callNext" => if on { self.decks[i].call_cue(1) },
            "jump" => if on { self.decks[i].beat_jump(num()) },
            "pfl" => if on { self.mixer.pfl[i] = !self.mixer.pfl[i]; self.send_mixer(); },
            "quant" => if on { self.decks[i].quant = !self.decks[i].quant },
            "keylock" => if on { let m = !self.decks[i].mt; self.set_mt(i, m); },
            "mode" => if on {
                match arg { "hotcue" => self.decks[i].pad_mode = PadMode::HotCue, "beatloop" => self.decks[i].pad_mode = PadMode::BeatLoop,
                    "beatjump" => self.decks[i].pad_mode = PadMode::BeatJump, _ => self.say(&format!("Pad mode \"{arg}\" isn't in Cratebox yet"), 3.0) }
            },
            "hot" => if on { self.decks[i].hotcue(num() as usize) },
            "hotClear" => if on { self.decks[i].clear_hot(num() as usize) },
            "loop" => if on { self.decks[i].beat_loop(num()) },
            "load" => if on { self.load_selected(i) },
            "browsePress" => if on { self.tree_press() },
            "browseShift" => if on { self.store.settings.browse = !self.store.settings.browse; self.store.dirty_settings = true; },
            "browseUp" => if on { self.list.move_sel(-1) },
            "browseDown" => if on { self.list.move_sel(1) },
            "unsupported" => if on { self.say(&format!("{arg} isn't in Cratebox yet"), 3.0) },
            _ => {}
        }
    }
    pub fn set_mt(&mut self, i: usize, on: bool) {
        self.decks[i].set_mt(on);
        self.store.settings.mt[i] = on;
        self.store.dirty_settings = true;
    }
    pub fn set_spin(&mut self, secs: f64) {
        self.store.settings.spin = secs;
        self.store.dirty_settings = true;
        for d in self.decks.iter_mut() { d.set_spin(secs); }
    }
    fn update_leds(&mut self) {
        if !self.midi_io.has_output() { self.midi.out.clear(); return; }
        let blink_on = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) % 1000) < 500;
        let decks = &self.decks;
        let pfl = self.mixer.pfl;
        self.midi.update_leds(|name, arg, i| {
            let d = &decks[i];
            Some(match name {
                "play" => d.playing || (d.loaded() && blink_on), // PLAY blinks when paused
                "cue" => { let at = d.loaded() && (d.now() - d.cue).abs() < 0.03; d.loaded() && (at || d.previewing) || (d.loaded() && !d.playing && blink_on) }
                "sync" => d.sync,
                "pfl" => pfl[i],
                "loopIn" => d.lp.on || (d.lp.a.is_some() && d.lp.b.is_none() && blink_on),
                "loopOut" => d.lp.on,
                "reloop" => d.lp.b.is_some(),
                "keylock" => d.mt,
                "quant" => d.quant,
                "mode" => match arg { "hotcue" => d.pad_mode == PadMode::HotCue, "beatloop" => d.pad_mode == PadMode::BeatLoop, "beatjump" => d.pad_mode == PadMode::BeatJump, _ => false },
                "hot" => d.pad_mode == PadMode::HotCue && arg.parse::<usize>().ok().and_then(|k| d.hot.get(k).copied().flatten()).is_some(),
                "loop" => d.pad_mode == PadMode::BeatLoop && d.lp.on && (d.loop_beats() - arg.parse::<f64>().unwrap_or(-1.0)).abs() < 0.01,
                _ => return None,
            })
        }, [level(self.status_snap.meters[0]), level(self.status_snap.meters[1])]);
    }

    // ======================= the per-frame tick =======================
    pub fn poll(&mut self) {
        // audio thread state
        if let Ok(s) = self.shared.lock() { self.status_snap = *s; }
        let snap = self.status_snap;
        for i in 0..2 { self.decks[i].on_report(&snap.decks[i], snap.at); }
        while self.trash_rx.try_recv().is_ok() {} // finished-with audio buffers get freed here, not on the audio thread
        if let Some(e) = self.audio.take_error() { self.say(&format!("Sound output problem: {e}"), 8.0); }
        // sync follows the other deck's tempo
        for i in 0..2 { let m = self.master(i); self.decks[i].sync_tick(m); self.decks[i].tick(); }
        // finished work
        while let Ok(j) = self.jobs_rx.try_recv() { self.on_job(j); }
        // deck side effects
        for i in 0..2 {
            let evs: Vec<DeckEvent> = std::mem::take(&mut self.decks[i].events);
            for e in evs { self.on_deck_event(e); }
        }
        self.run_queue();
        // controller
        if self.midi_io.maybe_rescan() { self.connect_midi(); }
        let msgs: Vec<Vec<u8>> = self.midi_io.rx.try_iter().collect();
        for m in msgs {
            if let Some(a) = self.midi.input(&m) { self.midi_action(a); }
        }
        if let Some(h) = self.midi.take_hint() { self.say(&h, 0.0); }
        if self.midi.learned_changed {
            self.midi.learned_changed = false;
            self.store.settings.learned.insert(self.midi.profile.to_string(), self.midi.learned.clone());
            self.store.dirty_settings = true;
            self.ui.learn_pick = None;
        }
        if self.last_led.elapsed() > Duration::from_millis(50) { self.last_led = Instant::now(); self.update_leds(); }
        if self.midi_io.has_output() && self.last_keep.elapsed() > Duration::from_millis(200) {
            self.last_keep = Instant::now();
            // the FLX4 drops into demo-light mode unless the software checks in every so often
            if let Some(k) = self.midi.profile_info().keep_alive { self.midi.send(k.to_vec()); }
        }
        for m in std::mem::take(&mut self.midi.out) { self.midi_io.send(&m); }
        // saving
        if self.lists.dirty {
            self.lists.dirty = false;
            self.store.playlists = self.lists.lists.clone();
            self.store.dirty_playlists = true;
            if matches!(self.view, View::Pl(_)) { self.apply_view(true); }
        }
        let open: Vec<String> = self.lists.open.iter().filter(|x| !x.starts_with("dev:")).cloned().collect();
        if open.len() != self.store.settings.pl_open.len() || open.iter().any(|x| !self.store.settings.pl_open.contains(x)) { self.store.settings.pl_open = open; self.store.dirty_settings = true; }
        if self.last_flush.elapsed() > Duration::from_secs(1) {
            self.last_flush = Instant::now();
            if let Err(e) = self.store.flush() { self.say(&format!("Couldn't save your library: {e}"), 8.0); }
        }
    }
    pub fn flush_now(&mut self) {
        self.store.playlists = self.lists.lists.clone();
        self.store.dirty_playlists = true;
        let _ = self.store.flush();
    }
    fn on_deck_event(&mut self, e: DeckEvent) {
        match e {
            DeckEvent::Status(s, secs) => self.say(&s, secs),
            DeckEvent::SaveCues(id, c) => { self.store.cues.insert(id, c); self.store.dirty_cues = true; }
            DeckEvent::SaveTrack(t) => { self.save_track(&t); self.apply_view(true); }
            DeckEvent::Rephrase { deck, mono, fs, bpm, first_beat, gen } => {
                let tx = self.jobs_tx.clone();
                let wake = self.repaint.clone();
                std::thread::spawn(move || {
                    let phrases = phrase_analysis(&mono, fs, bpm, first_beat);
                    let _ = tx.send(Job::Rephrased { deck, gen, phrases });
                    if let Some(w) = wake { w(); }
                });
            }
            DeckEvent::Reanalyse(id) => { if !self.queue.contains(&id) { self.queue.push_back(id); } }
            DeckEvent::Redraw => {}
        }
    }
    fn on_job(&mut self, j: Job) {
        match j {
            Job::Scanned { root, tracks, quiet } => {
                self.scanning = self.scanning.saturating_sub(1);
                if tracks.is_empty() && !quiet { self.say(&format!("No audio files found in {}", root.display()), 6.0); }
                self.add_tracks(tracks, quiet);
            }
            Job::Loaded { deck, gen, track, l, r, sr, ana, mono } => {
                let cues = match self.store.cues.get(&track.id) { Some(c) => c.clone().norm(), None => rb_cues_for(&track).unwrap_or_else(Cues::empty) };
                let id = track.id.clone();
                self.decks[deck].finish_load(gen, track, l, r, sr, ana, mono, cues);
                self.store.settings.bad.retain(|b| b != &id);
            }
            Job::LoadFailed { deck, gen, msg } => self.decks[deck].fail_load(gen, &msg),
            Job::Analysed { id, res } => {
                self.analysing = None;
                self.analysed_count += 1;
                let Some(&i) = self.by_id.get(&id) else { return };
                match res {
                    Ok(r) => {
                        let t = &mut self.tracks[i];
                        if !t.grid_manual { t.bpm = Some(fold_bpm(r.bpm, range_of(self.store.settings.bpm_range))); t.first_beat = Some(r.first_beat); }
                        if t.key_from.as_deref() != Some("rekordbox") || t.key.unwrap_or(-1) < 0 { t.key = Some(r.key); }
                        t.phrases = Some(r.phrases); t.duration = Some(r.duration); t.ver = crate::analysis::ANALYSIS_VER;
                        if t.artist.is_empty() { if let Some(a) = r.artist { t.artist = a; if let Some(ti) = r.title { t.title = ti; } } }
                        let c = Cached::of(t);
                        self.store.cache.insert(id.clone(), c);
                        self.store.dirty_cache = true;
                    }
                    Err(e) => {
                        let t = &mut self.tracks[i];
                        t.failed = true; t.bpm = t.bpm.or(Some(0.0)); t.key = t.key.or(Some(-1));
                        if !self.store.settings.bad.contains(&id) { self.store.settings.bad.push(id.clone()); self.store.dirty_settings = true; }
                        eprintln!("cratebox: couldn't analyse {}: {e}", t.path.display());
                    }
                }
                if self.analysed_count % 3 == 0 || self.queue.is_empty() { self.apply_view(true); }
            }
            Job::Rephrased { deck, gen, phrases } => self.decks[deck].rephrased(gen, phrases),
            Job::Mounted { root, res, remember } => match res {
                Ok((dev, tracks)) => {
                    let name = dev.name.clone();
                    let n = tracks.len();
                    let pls = dev.tree.iter().filter(|p| !p.folder).count();
                    let id = dev.id.clone();
                    self.lists.devices.retain(|x| x.id != id);
                    self.tracks.retain(|t| t.device.as_deref() != Some(&id));
                    self.reindex();
                    self.lists.open.insert(format!("dev:{id}"));
                    self.lists.devices.push(dev);
                    for t in tracks { self.by_id.insert(t.id.clone(), self.tracks.len()); self.tracks.push(t); }
                    if remember && !self.store.settings.devices.contains(&root) { self.store.settings.devices.push(root); self.store.dirty_settings = true; }
                    self.lists.rematch(&self.tracks);
                    self.apply_view(true);
                    self.say(&format!("{name}: {n} tracks, {pls} playlists (read-only)"), 6.0);
                }
                Err(e) => if remember { self.say(&e, 9.0) },
            },
        }
    }
}

/// meter level 0..1 from a peak (-42 dB .. 0 dB)
pub fn level(p: f32) -> f32 { ((20.0 * (p + 1e-9).log10() + 42.0) / 42.0).clamp(0.0, 1.0) }

impl Default for App { fn default() -> Self { App::new() } }
