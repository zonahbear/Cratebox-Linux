//! The track library: folder scanning, tags, the analysis cache, search/sort and the current view.

use crate::analysis::{fold_bpm, Phrase, ANALYSIS_VER};
use crate::keys::camelot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const AUDIO_EXT: [&str; 12] = ["mp3", "wav", "wave", "flac", "m4a", "aac", "ogg", "oga", "opus", "webm", "aif", "aiff"];
pub fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RbCue { pub hot: u32, pub ms: u32, pub loop_ms: Option<u32> }
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RbCues { pub hot: Vec<RbCue>, pub memory: Vec<RbCue> }

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    /// path relative to the music folder, starting with the folder's own name
    pub rel: String,
    pub size: u64,
    pub mtime: u64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub duration: Option<f64>,
    pub first_beat: Option<f64>,
    pub key: Option<i32>,
    pub phrases: Option<Vec<Phrase>>,
    pub ver: u32,
    pub grid_manual: bool,
    pub auto_bpm: Option<f64>,
    pub auto_first: Option<f64>,
    pub key_from: Option<String>,
    pub grid_from: Option<String>,
    /// rekordbox USB/SD tracks: the device id, rekordbox's analysis file and cues
    pub device: Option<String>,
    pub anlz_path: Option<PathBuf>,
    pub rb_read: bool,
    pub rb_cues: Option<RbCues>,
    /// playlist rows whose file can't be found right now
    pub missing: bool,
    pub failed: bool,
}

impl Track {
    pub fn analysis_current(&self) -> bool { self.bpm.is_some() && self.key.is_some() && self.phrases.is_some() && self.ver == ANALYSIS_VER }
    pub fn key_sort(&self) -> i32 { self.key.and_then(camelot).map(|c| c.sort()).unwrap_or(99) }
}

/// what's kept per track between runs (the analysis cache)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cached {
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub duration: Option<f64>,
    #[serde(rename = "firstBeat")]
    pub first_beat: Option<f64>,
    pub key: Option<i32>,
    pub ver: u32,
    #[serde(rename = "gridManual", default)]
    pub grid_manual: bool,
    #[serde(rename = "autoBpm")]
    pub auto_bpm: Option<f64>,
    #[serde(rename = "autoFirst")]
    pub auto_first: Option<f64>,
    pub phrases: Option<Vec<CachedPhrase>>,
    #[serde(rename = "keyFrom", default)]
    pub key_from: Option<String>,
    #[serde(rename = "gridFrom", default)]
    pub grid_from: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CachedPhrase { pub start: f64, pub bars: u32, pub label: String }

impl Cached {
    pub fn of(t: &Track) -> Cached {
        Cached {
            title: t.title.clone(), artist: t.artist.clone(), bpm: t.bpm, duration: t.duration, first_beat: t.first_beat, key: t.key, ver: t.ver,
            grid_manual: t.grid_manual, auto_bpm: t.auto_bpm, auto_first: t.auto_first,
            phrases: t.phrases.as_ref().map(|v| v.iter().map(|p| CachedPhrase { start: p.start, bars: p.bars, label: p.label.clone() }).collect()),
            key_from: t.key_from.clone(), grid_from: t.grid_from.clone(),
        }
    }
    pub fn apply(&self, t: &mut Track, range: (f64, f64)) {
        t.title = self.title.clone(); t.artist = self.artist.clone();
        t.bpm = self.bpm; t.duration = self.duration; t.first_beat = self.first_beat; t.key = self.key; t.ver = self.ver;
        t.grid_manual = self.grid_manual; t.auto_bpm = self.auto_bpm; t.auto_first = self.auto_first;
        t.phrases = self.phrases.as_ref().map(|v| v.iter().map(|p| Phrase { start: p.start, bars: p.bars, label: p.label.clone() }).collect());
        t.key_from = self.key_from.clone(); t.grid_from = self.grid_from.clone();
        if !t.grid_manual { t.bpm = t.bpm.map(|b| fold_bpm(b, range)); }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Cues { pub cue: Option<f64>, pub hot: Vec<Option<f64>> }
impl Cues {
    pub fn empty() -> Cues { Cues { cue: None, hot: vec![None; 8] } }
    pub fn norm(mut self) -> Cues { self.hot.resize(8, None); self }
}

pub fn parse_name(file: &str) -> (String, String) {
    let base = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file).replace('_', " ");
    // "01 - Artist - Title", "Artist - Title"
    let trimmed = base.trim();
    let no_num = {
        let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if (1..=3).contains(&digits) {
            let rest = &trimmed[digits..];
            let after = rest.trim_start_matches(|c| c == '.' || c == '-' || c == ' ');
            if after.len() < rest.len() && after.contains(" - ") { after.to_string() } else { trimmed.to_string() }
        } else { trimmed.to_string() }
    };
    if let Some(i) = no_num.find(" - ") {
        let (a, t) = (no_num[..i].trim(), no_num[i + 3..].trim());
        if !a.is_empty() && !t.is_empty() { return (a.to_string(), t.to_string()); }
    }
    (String::new(), trimmed.to_string())
}

/// ID3v2 title/artist from MP3s (other formats: tags from the decoder, or "Artist - Title" names)
pub fn read_id3(path: &Path) -> Option<(String, String)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 10];
    f.read_exact(&mut head).ok()?;
    if &head[0..3] != b"ID3" { return None; }
    let ver = head[3];
    let size = ((head[6] as usize) << 21) | ((head[7] as usize) << 14) | ((head[8] as usize) << 7) | head[9] as usize;
    let mut b = vec![0u8; size.min(512 * 1024)];
    let got = f.read(&mut b).ok()?;
    b.truncate(got);
    let (mut title, mut artist) = (String::new(), String::new());
    let mut p = 0;
    while p + 10 < b.len() {
        let id = &b[p..p + 4];
        if !id.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) { break; }
        let fs = if ver >= 4 { ((b[p + 4] as usize) << 21) | ((b[p + 5] as usize) << 14) | ((b[p + 6] as usize) << 7) | b[p + 7] as usize }
                 else { ((b[p + 4] as usize) << 24) | ((b[p + 5] as usize) << 16) | ((b[p + 6] as usize) << 8) | b[p + 7] as usize };
        if fs == 0 || p + 10 + fs > b.len() { break; }
        if id == b"TIT2" || id == b"TPE1" {
            let enc = b[p + 10];
            let data = &b[p + 11..p + 10 + fs];
            let s = decode_text(enc, data);
            let s = s.replace('\0', " ").trim().to_string();
            if id == b"TIT2" { title = s; } else { artist = s; }
        }
        p += 10 + fs;
    }
    if title.is_empty() && artist.is_empty() { None } else { Some((artist, title)) }
}

fn decode_text(enc: u8, data: &[u8]) -> String {
    match enc {
        0 => data.iter().map(|&c| c as char).collect(),
        1 | 2 => {
            let mut d = data;
            let mut be = enc == 2;
            if d.len() >= 2 && d[0] == 0xff && d[1] == 0xfe { be = false; d = &d[2..]; } else if d.len() >= 2 && d[0] == 0xfe && d[1] == 0xff { be = true; d = &d[2..]; }
            let u: Vec<u16> = d.chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
            String::from_utf16_lossy(&u)
        }
        _ => String::from_utf8_lossy(data).to_string(),
    }
}

pub fn track_id(rel: &str, size: u64, mtime: u64) -> String { format!("{rel}|{size}|{mtime}") }

/// a track for a file on disk; `root` is the picked music folder
pub fn make_track(path: &Path, root: &Path) -> Option<Track> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
    let root_name = root.file_name().and_then(|s| s.to_str()).unwrap_or("music");
    let rel_in = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let rel = format!("{root_name}/{rel_in}");
    let name = path.file_name()?.to_string_lossy().to_string();
    let (artist, title) = parse_name(&name);
    Some(Track { id: track_id(&rel, md.len(), mtime), path: path.to_path_buf(), name, rel, size: md.len(), mtime, title, artist, ..Default::default() })
}

pub fn scan_dir(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let p = e.path();
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') { continue; }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() { stack.push(p); } else if is_audio(&p) { out.push(p); }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey { Title, Artist, Bpm, Key, Time }

/// the list the library shows: indices into the view's track list, with their own positions
#[derive(Default)]
pub struct ListState {
    pub query: String,
    pub sort: Option<SortKey>,
    pub dir: i32,
    /// tracks shown (cloned rows: library tracks or playlist "missing" rows)
    pub view: Vec<Track>,
    /// each row's position in the view's own order (playlist position)
    pub view_pos: Vec<usize>,
    pub sel: usize,
    pub sel_ids: std::collections::HashSet<String>,
    pub anchor: usize,
    pub scroll_to_sel: bool,
}

impl ListState {
    pub fn new() -> ListState { ListState { dir: 1, ..Default::default() } }
    /// apply search and sort to `base` (the current view's tracks, in its own order)
    pub fn apply(&mut self, base: Vec<Track>, keep_sel: bool) {
        let q = self.query.to_lowercase();
        let sel_id = self.view.get(self.sel).map(|t| t.id.clone());
        let mut v: Vec<(Track, usize)> = base.into_iter().enumerate().map(|(i, t)| (t, i)).filter(|(t, _)| {
            q.is_empty() || t.title.to_lowercase().contains(&q) || t.artist.to_lowercase().contains(&q)
                || t.key.and_then(camelot).map(|c| c.label().to_lowercase() == q).unwrap_or(false)
        }).collect();
        if let Some(k) = self.sort {
            let d = self.dir as f64;
            v.sort_by(|a, b| {
                let o = match k {
                    SortKey::Title => a.0.title.to_lowercase().cmp(&b.0.title.to_lowercase()),
                    SortKey::Artist => a.0.artist.to_lowercase().cmp(&b.0.artist.to_lowercase()),
                    SortKey::Bpm => a.0.bpm.unwrap_or(0.0).partial_cmp(&b.0.bpm.unwrap_or(0.0)).unwrap(),
                    SortKey::Key => a.0.key_sort().cmp(&b.0.key_sort()),
                    SortKey::Time => a.0.duration.unwrap_or(0.0).partial_cmp(&b.0.duration.unwrap_or(0.0)).unwrap(),
                };
                if d < 0.0 { o.reverse() } else { o }
            });
        }
        self.view_pos = v.iter().map(|x| x.1).collect();
        self.view = v.into_iter().map(|x| x.0).collect();
        if keep_sel {
            if let Some(id) = sel_id { self.sel = self.view.iter().position(|t| t.id == id).unwrap_or(0); }
        }
        self.sel = self.sel.min(self.view.len().saturating_sub(1));
    }
    pub fn move_sel(&mut self, d: i32) {
        if self.view.is_empty() { return; }
        self.sel_ids.clear();
        self.sel = (self.sel as i64 + d as i64).clamp(0, self.view.len() as i64 - 1) as usize;
        self.anchor = self.sel;
        self.scroll_to_sel = true;
    }
    /// click: select; ctrl: add/remove; shift: a range
    pub fn click(&mut self, i: usize, ctrl: bool, shift: bool) {
        let Some(t) = self.view.get(i) else { return };
        let id = t.id.clone();
        if ctrl {
            if self.sel_ids.is_empty() { if let Some(c) = self.view.get(self.sel) { self.sel_ids.insert(c.id.clone()); } }
            if !self.sel_ids.remove(&id) { self.sel_ids.insert(id); }
            self.anchor = i;
        } else if shift {
            self.sel_ids.clear();
            let (a, b) = (self.anchor.min(i), self.anchor.max(i));
            for k in a..=b { self.sel_ids.insert(self.view[k].id.clone()); }
        } else { self.sel_ids.clear(); self.anchor = i; }
        self.sel = i;
    }
    pub fn selected(&self) -> Vec<Track> {
        if self.sel_ids.is_empty() { return self.view.get(self.sel).cloned().into_iter().collect(); }
        self.view.iter().filter(|t| self.sel_ids.contains(&t.id)).cloned().collect()
    }
    pub fn selected_positions(&self) -> Vec<usize> {
        if self.sel_ids.is_empty() { return self.view_pos.get(self.sel).copied().into_iter().collect(); }
        self.view.iter().enumerate().filter(|(_, t)| self.sel_ids.contains(&t.id)).map(|(k, _)| self.view_pos[k]).collect()
    }
}

pub type CacheMap = HashMap<String, Cached>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert_eq!(parse_name("Bass Lab - Halftime Wobble 140.wav"), ("Bass Lab".into(), "Halftime Wobble 140".into()));
        assert_eq!(parse_name("03 - Jungle Unit - Rollers.mp3"), ("Jungle Unit".into(), "Rollers".into()));
        assert_eq!(parse_name("just_a_title.flac"), ("".into(), "just a title".into()));
    }
    #[test]
    fn list_search_sort_select() {
        let mk = |t: &str, b: f64| Track { id: t.into(), title: t.into(), bpm: Some(b), ..Default::default() };
        let mut l = ListState::new();
        l.apply(vec![mk("b", 140.0), mk("a", 126.0), mk("c", 174.0)], false);
        l.sort = Some(SortKey::Bpm);
        l.apply(vec![mk("b", 140.0), mk("a", 126.0), mk("c", 174.0)], false);
        assert_eq!(l.view.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(l.view_pos, [1, 0, 2]);
        l.click(0, false, false);
        l.click(2, false, true);
        assert_eq!(l.selected().len(), 3);
        l.query = "c".into();
        l.apply(vec![mk("b", 140.0), mk("a", 126.0), mk("c", 174.0)], true);
        assert_eq!(l.view.len(), 1);
    }
}
