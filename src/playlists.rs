//! Playlists (with folders), rekordbox USB/SD devices (read-only) and rekordbox XML import/export.
//! The app's view of these lives here; drawing them is in the UI.

use crate::analysis::fold_bpm;
use crate::keys::{key_index, key_xml};
use crate::library::{Cached, Cues, RbCue, RbCues, Track};
use crate::rekordbox::{grid_from_beats, location_url, parse_anlz, parse_pdb, parse_xml, phrases_from, xml_esc, PdbNode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta { pub title: String, pub artist: String, pub file: String, pub size: u64 }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub folder: bool,
    pub parent: Option<String>,
    pub order: u32,
    pub items: Vec<String>,
    /// lets a playlist show (and later re-find) tracks whose folder isn't connected right now
    pub meta: HashMap<String, Meta>,
}

pub fn meta_of(t: &Track) -> Meta { Meta { title: t.title.clone(), artist: t.artist.clone(), file: t.name.clone(), size: t.size } }

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("pl{:x}{:04x}", t, N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) & 0xffff)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View { All, Pl(String), Dev(String), DevPl(String, u32) }

/// a mounted rekordbox USB/SD export
#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub tree: Vec<PdbNode>,
    /// rekordbox track id -> Cratebox track id
    pub by_rb: HashMap<u32, String>,
    pub track_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind { All, Head(&'static str), Pl, Folder, Dev, DevPl, DevFolder }

#[derive(Clone, Debug)]
pub struct TreeRow { pub kind: RowKind, pub id: String, pub dev: String, pub rb: u32, pub depth: usize, pub label: String, pub count: Option<usize> }

#[derive(Default)]
pub struct Lists {
    pub lists: Vec<Playlist>,
    pub devices: Vec<Device>,
    pub open: HashSet<String>,
    pub dirty: bool,
}

impl Lists {
    pub fn get(&self, id: &str) -> Option<&Playlist> { self.lists.iter().find(|p| p.id == id) }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Playlist> { self.lists.iter_mut().find(|p| p.id == id) }

    pub fn create(&mut self, name: &str, folder: bool, parent: Option<String>) -> String {
        let order = self.lists.iter().filter(|x| x.parent == parent).count() as u32;
        let id = new_id();
        if let Some(p) = &parent { self.open.insert(p.clone()); }
        self.lists.push(Playlist { id: id.clone(), name: name.into(), folder, parent, order, items: vec![], meta: HashMap::new() });
        self.dirty = true;
        id
    }
    pub fn next_name(&self, base: &str) -> String {
        let mut n = 1;
        while self.lists.iter().any(|x| x.name == format!("{base} {n}")) { n += 1; }
        format!("{base} {n}")
    }
    /// add tracks at `at` (None = the end); returns how many went in
    pub fn add(&mut self, id: &str, tracks: &[Track], at: Option<usize>) -> usize {
        let Some(p) = self.get_mut(id) else { return 0 };
        if p.folder { return 0; }
        let ok: Vec<&Track> = tracks.iter().filter(|t| !t.missing).collect();
        for t in &ok { p.meta.insert(t.id.clone(), meta_of(t)); }
        let at = at.unwrap_or(p.items.len()).min(p.items.len());
        let ids: Vec<String> = ok.iter().map(|t| t.id.clone()).collect();
        let n = ids.len();
        p.items.splice(at..at, ids);
        self.dirty = true;
        n
    }
    pub fn remove_at(&mut self, id: &str, positions: &[usize]) {
        let Some(p) = self.get_mut(id) else { return };
        let drop: HashSet<usize> = positions.iter().copied().collect();
        p.items = p.items.iter().enumerate().filter(|(i, _)| !drop.contains(i)).map(|(_, x)| x.clone()).collect();
        self.dirty = true;
    }
    /// move the items at `positions` so they start at index `to` (counted in the list before removal);
    /// returns the new positions of the moved items
    pub fn move_items(&mut self, id: &str, positions: &[usize], to: usize) -> Vec<usize> {
        let Some(p) = self.get_mut(id) else { return vec![] };
        let mut pos = positions.to_vec();
        pos.sort();
        pos.dedup();
        let moving: Vec<String> = pos.iter().filter_map(|&i| p.items.get(i).cloned()).collect();
        let before = pos.iter().filter(|&&i| i < to).count();
        let mut rest: Vec<String> = p.items.iter().enumerate().filter(|(i, _)| !pos.contains(i)).map(|(_, x)| x.clone()).collect();
        let at = to.saturating_sub(before).min(rest.len());
        let n = moving.len();
        rest.splice(at..at, moving);
        p.items = rest;
        self.dirty = true;
        (at..at + n).collect()
    }
    pub fn delete(&mut self, id: &str) -> HashSet<String> {
        let mut kill: HashSet<String> = [id.to_string()].into();
        loop {
            let more: Vec<String> = self.lists.iter().filter(|x| x.parent.as_ref().is_some_and(|p| kill.contains(p)) && !kill.contains(&x.id)).map(|x| x.id.clone()).collect();
            if more.is_empty() { break; }
            kill.extend(more);
        }
        self.lists.retain(|x| !kill.contains(&x.id));
        self.dirty = true;
        kill
    }
    pub fn rename(&mut self, id: &str, name: &str) {
        if let Some(p) = self.get_mut(id) { if !name.trim().is_empty() { p.name = name.trim().to_string(); self.dirty = true; } }
    }
    /// file a playlist/folder under `parent` (None = top level); no moving a folder into itself
    pub fn move_to(&mut self, id: &str, parent: Option<String>) -> bool {
        let mut x = parent.clone();
        while let Some(cur) = x { if cur == id { return false; } x = self.get(&cur).and_then(|p| p.parent.clone()); }
        let order = self.lists.iter().filter(|x| x.parent == parent && x.id != id).count() as u32;
        if let Some(p) = &parent { self.open.insert(p.clone()); }
        let Some(p) = self.get_mut(id) else { return false };
        p.parent = parent; p.order = order;
        self.dirty = true;
        true
    }
    pub fn path_name(&self, p: &Playlist) -> String {
        let mut parts = vec![p.name.clone()];
        let mut x = p.parent.clone();
        while let Some(id) = x { match self.get(&id) { Some(q) => { parts.insert(0, q.name.clone()); x = q.parent.clone(); } None => break } }
        parts.join(" / ")
    }

    /// a playlist's rows; tracks we can't find show as "missing" rows (title from the saved meta)
    pub fn tracks(&self, id: &str, by_id: &HashMap<String, usize>, lib: &[Track]) -> Vec<Track> {
        let Some(p) = self.get(id) else { return vec![] };
        p.items.iter().map(|tid| match by_id.get(tid) {
            Some(&i) => lib[i].clone(),
            None => {
                let m = p.meta.get(tid).cloned().unwrap_or_default();
                let title = if m.title.is_empty() { tid.split('|').next().unwrap_or(tid).rsplit('/').next().unwrap_or(tid).to_string() } else { m.title };
                Track { id: tid.clone(), title, artist: m.artist, name: m.file, size: m.size, missing: true, ..Default::default() }
            }
        }).collect()
    }
    /// a folder was (re)connected: point missing entries at tracks with the same file name and size
    pub fn rematch(&mut self, lib: &[Track]) {
        let mut by_file: HashMap<String, usize> = HashMap::new();
        for (i, t) in lib.iter().enumerate() {
            let f = t.name.to_lowercase();
            by_file.insert(format!("{f}|{}", t.size), i);
            by_file.entry(f).or_insert(i);
        }
        let have: HashSet<&str> = lib.iter().map(|t| t.id.as_str()).collect();
        let mut changed = false;
        for p in self.lists.iter_mut() {
            let mut add_meta = vec![];
            for item in p.items.iter_mut() {
                if have.contains(item.as_str()) { continue; }
                let Some(m) = p.meta.get(item.as_str()) else { continue };
                if m.file.is_empty() { continue; }
                let f = m.file.to_lowercase();
                if let Some(&i) = by_file.get(&format!("{f}|{}", m.size)).or_else(|| by_file.get(&f)) {
                    *item = lib[i].id.clone();
                    add_meta.push(i);
                    changed = true;
                }
            }
            for i in add_meta { p.meta.insert(lib[i].id.clone(), meta_of(&lib[i])); }
        }
        if changed { self.dirty = true; }
    }

    // ---- the tree ----
    pub fn tree_rows(&self, collection_count: usize) -> Vec<TreeRow> {
        let row = |kind, id: &str, depth, label: &str, count| TreeRow { kind, id: id.into(), dev: String::new(), rb: 0, depth, label: label.into(), count };
        let mut rows = vec![row(RowKind::All, "", 0, "Collection", Some(collection_count)), row(RowKind::Head("pl"), "", 0, "PLAYLISTS", None)];
        fn walk(l: &Lists, parent: Option<&String>, depth: usize, rows: &mut Vec<TreeRow>) {
            let mut kids: Vec<&Playlist> = l.lists.iter().filter(|x| x.parent.as_ref() == parent).collect();
            kids.sort_by(|a, b| a.order.cmp(&b.order).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            for p in kids {
                rows.push(TreeRow { kind: if p.folder { RowKind::Folder } else { RowKind::Pl }, id: p.id.clone(), dev: String::new(), rb: 0, depth, label: p.name.clone(),
                    count: if p.folder { None } else { Some(p.items.len()) } });
                if p.folder && l.open.contains(&p.id) { walk(l, Some(&p.id), depth + 1, rows); }
            }
        }
        walk(self, None, 1, &mut rows);
        rows.push(row(RowKind::Head("dev"), "", 0, "DEVICES", None));
        for d in &self.devices {
            rows.push(TreeRow { kind: RowKind::Dev, id: String::new(), dev: d.id.clone(), rb: 0, depth: 1, label: d.name.clone(), count: Some(d.track_count) });
            if !self.open.contains(&format!("dev:{}", d.id)) { continue; }
            fn dw(l: &Lists, d: &Device, parent: u32, depth: usize, rows: &mut Vec<TreeRow>) {
                let mut kids: Vec<&PdbNode> = d.tree.iter().filter(|x| x.parent == parent).collect();
                kids.sort_by_key(|x| x.sort);
                for n in kids {
                    rows.push(TreeRow { kind: if n.folder { RowKind::DevFolder } else { RowKind::DevPl }, id: String::new(), dev: d.id.clone(), rb: n.id, depth, label: n.name.clone(),
                        count: if n.folder { None } else { Some(n.tracks.len()) } });
                    if n.folder && l.open.contains(&format!("dev:{}:{}", d.id, n.id)) { dw(l, d, n.id, depth + 1, rows); }
                }
            }
            dw(self, d, 0, 2, &mut rows);
        }
        rows
    }
    pub fn open_key(r: &TreeRow) -> String {
        match r.kind { RowKind::Folder => r.id.clone(), RowKind::Dev => format!("dev:{}", r.dev), _ => format!("dev:{}:{}", r.dev, r.rb) }
    }
    pub fn toggle_open(&mut self, r: &TreeRow) {
        let k = Self::open_key(r);
        if !self.open.remove(&k) { self.open.insert(k); }
    }
    pub fn view_name(&self, v: &View) -> String {
        match v {
            View::All => "Collection".into(),
            View::Pl(id) => self.get(id).map(|p| p.name.clone()).unwrap_or_else(|| "Playlist".into()),
            View::Dev(d) => self.devices.iter().find(|x| &x.id == d).map(|x| x.name.clone()).unwrap_or_else(|| "Device".into()),
            View::DevPl(d, n) => {
                let Some(dev) = self.devices.iter().find(|x| &x.id == d) else { return "Device".into() };
                format!("{} · {}", dev.name, dev.tree.iter().find(|x| x.id == *n).map(|x| x.name.as_str()).unwrap_or(""))
            }
        }
    }
}

pub fn row_is_view(r: &TreeRow, v: &View) -> bool {
    match (&r.kind, v) {
        (RowKind::All, View::All) => true,
        (RowKind::Pl, View::Pl(id)) => &r.id == id,
        (RowKind::Dev, View::Dev(d)) => &r.dev == d,
        (RowKind::DevPl, View::DevPl(d, n)) => &r.dev == d && r.rb == *n,
        _ => false,
    }
}

// ---- rekordbox USB / SD ----
fn sub(root: &Path, p: &str) -> PathBuf {
    // rekordbox paths start with "/" and use "/"; Linux filesystems on FAT/exFAT sticks may differ in case
    let rel = p.trim_start_matches('/');
    let direct = root.join(rel);
    if direct.exists() { return direct; }
    let mut cur = root.to_path_buf();
    for part in rel.split('/') {
        let exact = cur.join(part);
        if exact.exists() { cur = exact; continue; }
        let found = std::fs::read_dir(&cur).ok().and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part)).map(|e| e.path()));
        match found { Some(f) => cur = f, None => return direct }
    }
    cur
}

/// read a rekordbox export (the drive root, the folder holding PIONEER): its tracks and playlist tree
pub fn mount_device(root: &Path, saved: &crate::library::CacheMap, range: (f64, f64)) -> Result<(Device, Vec<Track>), String> {
    let name = root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| root.display().to_string());
    let pdb_path = sub(root, "/PIONEER/rekordbox/export.pdb");
    let buf = std::fs::read(&pdb_path).map_err(|_| format!("\"{name}\" doesn't look like a rekordbox USB/SD export (no PIONEER/rekordbox/export.pdb). Pick the drive itself."))?;
    let db = parse_pdb(&buf).map_err(|e| format!("Couldn't read that rekordbox database: {e}"))?;
    let id = format!("usb-{name}");
    let mut dev = Device { id: id.clone(), name: name.clone(), root: root.to_path_buf(), tree: db.playlists, by_rb: HashMap::new(), track_count: db.tracks.len() };
    let mut out = vec![];
    for r in db.tracks {
        let tid = format!("{id}|{}", r.path);
        let path = sub(root, &r.path);
        let key = key_index(&r.key);
        let mut t = Track {
            id: tid.clone(), path, name: r.filename.clone(), rel: r.path.clone(), size: r.size as u64, title: if r.title.is_empty() { r.filename.clone() } else { r.title.clone() },
            artist: r.artist.clone(), bpm: if r.bpm > 0.0 { Some(fold_bpm(r.bpm, range)) } else { None }, duration: Some(r.duration as f64), key: Some(key),
            phrases: Some(vec![]), ver: crate::analysis::ANALYSIS_VER, device: Some(id.clone()),
            anlz_path: if r.anlz_path.is_empty() { None } else { Some(sub(root, &r.anlz_path)) }, ..Default::default()
        };
        // your own edits to these tracks (grid fixes) from earlier sessions
        if let Some(c) = saved.get(&tid) {
            if c.grid_manual { t.bpm = c.bpm; t.first_beat = c.first_beat; t.grid_manual = true; t.auto_bpm = c.auto_bpm; t.auto_first = c.auto_first; }
        }
        dev.by_rb.insert(r.id, tid);
        out.push(t);
    }
    Ok((dev, out))
}

/// rekordbox's own analysis for a device track: grid, phrases, hot cues and memory cues
pub fn read_device_analysis(t: &mut Track) -> bool {
    let Some(dat_path) = t.anlz_path.clone() else { return false };
    let Ok(dat) = std::fs::read(&dat_path).map(|b| parse_anlz(&b)) else { return false };
    if dat.beats.is_empty() { return false; }
    let ext = std::fs::read(dat_path.with_extension("EXT")).or_else(|_| std::fs::read(dat_path.with_extension("ext"))).ok().map(|b| parse_anlz(&b));
    let Some((bpm, first)) = grid_from_beats(&dat.beats) else { return false };
    if !t.grid_manual { t.bpm = Some(bpm); t.first_beat = Some(first); t.auto_bpm = Some(bpm); t.auto_first = Some(first); }
    t.phrases = Some(ext.as_ref().and_then(|e| e.phrases.as_ref()).and_then(|p| phrases_from(&dat.beats, p)).unwrap_or_default());
    let src = match &ext { Some(e) if !e.hot.is_empty() || !e.memory.is_empty() => e, _ => &dat };
    t.rb_cues = Some(RbCues { hot: src.hot.clone(), memory: src.memory.clone() });
    t.rb_read = true;
    true
}

/// rekordbox hot cues A-H and the first memory cue, used until you set your own in Cratebox
pub fn rb_cues_for(t: &Track) -> Option<Cues> {
    let c = t.rb_cues.as_ref()?;
    let mut hot = vec![None; 8];
    for q in &c.hot { if (1..=8).contains(&q.hot) { hot[q.hot as usize - 1] = Some(q.ms as f64 / 1000.0); } }
    let mem: Option<&RbCue> = c.memory.iter().min_by_key(|m| m.ms);
    Some(Cues { cue: mem.map(|m| m.ms as f64 / 1000.0), hot })
}

// ---- rekordbox XML ----
pub struct XmlImport {
    pub tracks: usize,
    pub matched: usize,
    pub applied: usize,
    pub playlists: usize,
    /// (track index in lib, changes applied) and cues to set where you have none
    pub cue_sets: Vec<(String, Cues)>,
}

/// take rekordbox's grid, key and cues for a track (your own Cratebox edits win)
pub fn apply_rekordbox_track(t: &mut Track, r: &crate::rekordbox::XmlTrack, have_cues: bool) -> (bool, Option<Cues>) {
    let mut did = false;
    if let Some((bpm, inizio, battito)) = r.grid {
        if bpm > 0.0 && !t.grid_manual {
            let spb = 60.0 / bpm;
            let mut first = inizio - (battito.max(1) as f64 - 1.0) * spb; // back to the downbeat
            let bar = 4.0 * spb;
            first -= (first / bar).floor() * bar;
            if t.auto_bpm.is_none() { t.auto_bpm = t.bpm.or(Some(bpm)); t.auto_first = t.first_beat.or(Some(first)); }
            t.bpm = Some(bpm); t.first_beat = Some(first); t.grid_manual = true; t.grid_from = Some("rekordbox".into());
            did = true;
        }
    }
    let k = key_index(&r.key);
    if k >= 0 { t.key = Some(k); t.key_from = Some("rekordbox".into()); did = true; }
    if t.duration.is_none() && r.duration > 0.0 { t.duration = Some(r.duration); }
    let mut cues = None;
    if !r.marks.is_empty() && !have_cues {
        let mut hot = vec![None; 8];
        for m in &r.marks { if (0..8).contains(&m.num) { hot[m.num as usize] = Some(m.start); } }
        let mem = r.marks.iter().filter(|m| m.num == -1).map(|m| m.start).fold(None, |a: Option<f64>, s| Some(a.map_or(s, |a| a.min(s))));
        cues = Some(Cues { cue: mem, hot });
        did = true;
    }
    (did, cues)
}

/// import: match rekordbox's tracks to files Cratebox knows (file name + size, then file name);
/// their playlists go into a "rekordbox (date)" folder. Returns what happened, for the status line.
pub fn import_xml(text: &str, lib: &mut [Track], cues: &HashMap<String, Cues>, lists: &mut Lists, date: &str) -> Result<XmlImport, String> {
    let x = parse_xml(text)?;
    let mut by_file: HashMap<String, usize> = HashMap::new();
    for (i, t) in lib.iter().enumerate() {
        if t.device.is_some() { continue; }
        let f = t.name.to_lowercase();
        by_file.insert(format!("{f}|{}", t.size), i);
        by_file.entry(f).or_insert(i);
    }
    let mut out = XmlImport { tracks: x.tracks.len(), matched: 0, applied: 0, playlists: x.playlists.iter().filter(|p| !p.folder).count(), cue_sets: vec![] };
    let mut resolved: HashMap<String, (String, Meta)> = HashMap::new();
    for r in &x.tracks {
        let f = r.filename.to_lowercase();
        match by_file.get(&format!("{f}|{}", r.size)).or_else(|| by_file.get(&f)).copied() {
            Some(i) => {
                out.matched += 1;
                let have = cues.contains_key(&lib[i].id);
                let (did, c) = apply_rekordbox_track(&mut lib[i], r, have);
                if did { out.applied += 1; }
                if let Some(c) = c { out.cue_sets.push((lib[i].id.clone(), c)); }
                resolved.insert(r.id.clone(), (lib[i].id.clone(), meta_of(&lib[i])));
            }
            None => {
                let id = format!("rbx|{}|{}", r.filename, r.size);
                resolved.insert(r.id.clone(), (id, Meta { title: r.title.clone(), artist: r.artist.clone(), file: r.filename.clone(), size: r.size }));
            }
        }
    }
    let top = lists.create(&format!("rekordbox ({date})"), true, None);
    let mut made: HashMap<usize, String> = HashMap::new();
    for n in &x.playlists {
        let parent = match n.parent { None => top.clone(), Some(k) => made.get(&k).cloned().unwrap_or(top.clone()) };
        let id = lists.create(&n.name, n.folder, Some(parent));
        made.insert(n.key, id.clone());
        if !n.folder {
            let p = lists.get_mut(&id).unwrap();
            for rid in &n.tracks {
                if let Some((tid, m)) = resolved.get(rid) { p.items.push(tid.clone()); p.meta.insert(tid.clone(), m.clone()); }
            }
        }
    }
    lists.open.insert(top);
    Ok(out)
}

/// export every collection track (grid, key, cues) and all playlists as rekordbox XML.
/// `base`: where the music folders are on this computer (normally their real paths, so empty is fine)
pub fn export_xml(lib: &[Track], cues: &HashMap<String, Cues>, lists: &Lists, base: &str) -> String {
    let own: Vec<&Track> = lib.iter().filter(|t| t.device.is_none()).collect();
    let tid: HashMap<&str, usize> = own.iter().enumerate().map(|(i, t)| (t.id.as_str(), i + 1)).collect();
    let loc = |t: &Track| -> String {
        if base.is_empty() { return location_url(&t.path.to_string_lossy()); }
        let rel: String = t.rel.splitn(2, '/').nth(1).unwrap_or(&t.rel).to_string();
        location_url(&format!("{}/{}", base.trim_end_matches(['/', '\\']), rel))
    };
    let mut out = vec![
        r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string(),
        r#"<DJ_PLAYLISTS Version="1.0.0">"#.into(),
        r#"  <PRODUCT Name="Cratebox" Version="1" Company="Cratebox"/>"#.into(),
        format!(r#"  <COLLECTION Entries="{}">"#, own.len()),
    ];
    for (i, t) in own.iter().enumerate() {
        let kind = t.name.rsplit('.').next().unwrap_or("").to_uppercase() + " File";
        out.push(format!(r#"    <TRACK TrackID="{}" Name="{}" Artist="{}" Kind="{}" Size="{}" TotalTime="{}" AverageBpm="{:.2}" Tonality="{}" Location="{}">"#,
            i + 1, xml_esc(&t.title), xml_esc(&t.artist), xml_esc(&kind), t.size, t.duration.unwrap_or(0.0).round() as i64, t.bpm.unwrap_or(0.0),
            key_xml(t.key.unwrap_or(-1)), xml_esc(&loc(t))));
        if let (Some(b), Some(f)) = (t.bpm, t.first_beat) { out.push(format!(r#"      <TEMPO Inizio="{f:.3}" Bpm="{b:.2}" Metro="4/4" Battito="1"/>"#)); }
        if let Some(c) = cues.get(&t.id) {
            if let Some(q) = c.cue { out.push(format!(r#"      <POSITION_MARK Name="" Type="0" Start="{q:.3}" Num="-1"/>"#)); }
            for (k, h) in c.hot.iter().enumerate() { if let Some(v) = h { out.push(format!(r#"      <POSITION_MARK Name="" Type="0" Start="{v:.3}" Num="{k}"/>"#)); } }
        }
        out.push("    </TRACK>".into());
    }
    out.push("  </COLLECTION>".into());
    out.push("  <PLAYLISTS>".into());
    fn node(l: &Lists, parent: Option<&String>, ind: &str, tid: &HashMap<&str, usize>, out: &mut Vec<String>) {
        let mut kids: Vec<&Playlist> = l.lists.iter().filter(|x| x.parent.as_ref() == parent).collect();
        kids.sort_by_key(|x| x.order);
        for p in kids {
            if p.folder {
                out.push(format!(r#"{ind}<NODE Type="0" Name="{}" Count="{}">"#, xml_esc(&p.name), l.lists.iter().filter(|x| x.parent.as_ref() == Some(&p.id)).count()));
                node(l, Some(&p.id), &format!("{ind}  "), tid, out);
                out.push(format!("{ind}</NODE>"));
            } else {
                let items: Vec<usize> = p.items.iter().filter_map(|i| tid.get(i.as_str()).copied()).collect();
                out.push(format!(r#"{ind}<NODE Name="{}" Type="1" KeyType="0" Entries="{}">"#, xml_esc(&p.name), items.len()));
                for k in items { out.push(format!(r#"{ind}  <TRACK Key="{k}"/>"#)); }
                out.push(format!("{ind}</NODE>"));
            }
        }
    }
    out.push(format!(r#"    <NODE Type="0" Name="ROOT" Count="{}">"#, lists.lists.iter().filter(|x| x.parent.is_none()).count()));
    node(lists, None, "      ", &tid, &mut out);
    out.push("    </NODE>".into());
    out.push("  </PLAYLISTS>".into());
    out.push("</DJ_PLAYLISTS>".into());
    out.join("\n")
}

/// write rekordbox's analysis into the cache record of a track (used after XML import)
pub fn cached_of(t: &Track) -> Cached { Cached::of(t) }

#[cfg(test)]
mod tests {
    use super::*;
    fn tr(id: &str, name: &str, size: u64) -> Track { Track { id: id.into(), name: name.into(), title: name.into(), size, rel: format!("music/{name}"), path: PathBuf::from(format!("/m/{name}")), ..Default::default() } }
    #[test]
    fn add_move_remove_delete() {
        let mut l = Lists::default();
        let p = l.create("Bangers", false, None);
        let lib = vec![tr("a", "a.wav", 1), tr("b", "b.wav", 2), tr("c", "c.wav", 3)];
        assert_eq!(l.add(&p, &lib, None), 3);
        assert_eq!(l.move_items(&p, &[0], 3), vec![2]);
        assert_eq!(l.get(&p).unwrap().items, ["b", "c", "a"]);
        l.remove_at(&p, &[1]);
        assert_eq!(l.get(&p).unwrap().items, ["b", "a"]);
        let f = l.create("Sets", true, None);
        assert!(l.move_to(&p, Some(f.clone())));
        assert!(!l.move_to(&f, Some(p.clone())) || l.get(&f).unwrap().parent.as_deref() == Some(&p));
        let rows = l.tree_rows(3);
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Collection", "PLAYLISTS", "Sets", "Bangers", "DEVICES"]);
        l.delete(&f);
        assert!(l.lists.is_empty());
    }
    #[test]
    fn missing_rows_and_rematch() {
        let mut l = Lists::default();
        let p = l.create("x", false, None);
        let old = vec![tr("old|a", "a.wav", 5)];
        l.add(&p, &old, None);
        let lib = vec![tr("new|a", "a.wav", 5)];
        let by: HashMap<String, usize> = lib.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();
        assert!(l.tracks(&p, &by, &lib)[0].missing);
        l.rematch(&lib);
        assert!(!l.tracks(&p, &by, &lib)[0].missing);
    }
    #[test]
    fn xml_round_trip() {
        let mut lib = vec![tr("t1", "House Groove 126.wav", 1000), tr("t2", "Other.wav", 9)];
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><DJ_PLAYLISTS Version="1.0.0"><COLLECTION Entries="2">
<TRACK TrackID="11" Name="House Groove" Artist="A" Size="1000" TotalTime="60" AverageBpm="126.00" Tonality="Am" Location="file://localhost/C:/Music/House%20Groove%20126.wav">
<TEMPO Inizio="0.367" Bpm="126.00" Metro="4/4" Battito="1"/><POSITION_MARK Name="" Type="0" Start="15.6" Num="0"/><POSITION_MARK Type="0" Start="0.367" Num="-1"/></TRACK>
<TRACK TrackID="12" Name="Ghost" Artist="N" Size="5" Location="file://localhost/C:/Music/ghost.mp3"/></COLLECTION>
<PLAYLISTS><NODE Type="0" Name="ROOT" Count="1"><NODE Type="0" Name="Bass" Count="1"><NODE Name="Wobbles" Type="1" KeyType="0" Entries="2"><TRACK Key="11"/><TRACK Key="12"/></NODE></NODE></NODE></PLAYLISTS></DJ_PLAYLISTS>"#;
        let mut l = Lists::default();
        let r = import_xml(xml, &mut lib, &HashMap::new(), &mut l, "today").unwrap();
        assert_eq!((r.tracks, r.matched, r.applied, r.playlists), (2, 1, 1, 1));
        assert_eq!(lib[0].bpm, Some(126.0));
        assert!((lib[0].first_beat.unwrap() - 0.367).abs() < 1e-9 && lib[0].grid_manual);
        assert_eq!(lib[0].key, Some(21));
        assert_eq!(r.cue_sets[0].1.hot[0], Some(15.6));
        let w = l.lists.iter().find(|p| p.name == "Wobbles").unwrap();
        assert_eq!(w.items.len(), 2);
        let bass = l.get(w.parent.as_ref().unwrap()).unwrap();
        assert_eq!(bass.name, "Bass");
        assert_eq!(l.get(bass.parent.as_ref().unwrap()).unwrap().name, "rekordbox (today)");
        let cues: HashMap<String, Cues> = r.cue_sets.into_iter().collect();
        let out = export_xml(&lib, &cues, &l, "");
        assert!(out.contains(r#"Location="file://localhost/m/House%20Groove%20126.wav""#), "{out}");
        assert!(out.contains(r#"<TEMPO Inizio="0.367" Bpm="126.00""#));
        assert!(out.contains(r#"Start="15.600" Num="0""#));
        assert!(out.contains(r#"<NODE Name="Wobbles" Type="1" KeyType="0" Entries="1">"#)); // the missing one is left out
        let back = parse_xml(&out).unwrap();
        assert_eq!(back.tracks.len(), 2);
    }
    #[test]
    fn usb_fixture_mounts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rekordbox_usb");
        let (dev, mut tracks) = mount_device(&root, &HashMap::new(), (88.0, 176.0)).unwrap();
        assert_eq!(dev.track_count, 6);
        let mut l = Lists::default();
        l.devices.push(dev.clone());
        l.open.insert(format!("dev:{}", dev.id));
        let rows = l.tree_rows(0);
        assert!(rows.iter().any(|r| r.label == "Playlist 1") && rows.iter().any(|r| r.label == "Folder"));
        let t = tracks.iter_mut().find(|t| t.title == "Demo Track 1").unwrap();
        assert!(read_device_analysis(t));
        assert_eq!((t.bpm, t.first_beat), (Some(128.0), Some(0.025)));
        assert_eq!(t.phrases.as_ref().unwrap().len(), 19);
    }
}
