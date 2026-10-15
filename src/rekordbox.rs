//! rekordbox: USB/SD exports (export.pdb + ANLZ files) and rekordbox XML.
//! export.pdb ("Device Library") and ANLZ layouts follow Deep Symmetry's reverse-engineering in
//! crate-digger (rekordbox_pdb.ksy / rekordbox_anlz.ksy). Read-only: Cratebox never writes to a stick.

use crate::analysis::Phrase;
use crate::library::{RbCue, RbCues};
use std::collections::HashMap;

struct Rd<'a>(&'a [u8]);
impl<'a> Rd<'a> {
    fn u8(&self, p: usize) -> Option<u8> { self.0.get(p).copied() }
    fn u16le(&self, p: usize) -> Option<u16> { Some(u16::from_le_bytes(self.0.get(p..p + 2)?.try_into().ok()?)) }
    fn u32le(&self, p: usize) -> Option<u32> { Some(u32::from_le_bytes(self.0.get(p..p + 4)?.try_into().ok()?)) }
    fn u16be(&self, p: usize) -> Option<u16> { Some(u16::from_be_bytes(self.0.get(p..p + 2)?.try_into().ok()?)) }
    fn u32be(&self, p: usize) -> Option<u32> { Some(u32::from_be_bytes(self.0.get(p..p + 4)?.try_into().ok()?)) }
    /// DeviceSQL string: short ASCII, long ASCII (0x40) or long UTF-16LE (0x90)
    fn string(&self, pos: usize) -> Option<String> {
        let lk = self.u8(pos)?;
        if lk == 0x40 || lk == 0x90 {
            let len = self.u16le(pos + 1)? as usize;
            let bytes = self.0.get(pos + 4..pos + 4 + len.saturating_sub(4))?;
            let s = if lk == 0x40 { bytes.iter().map(|&c| c as char).collect::<String>() }
                else { String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()) };
            return Some(s.trim_end_matches('\0').to_string());
        }
        let n = (lk >> 1) as i32 - 1;
        if n <= 0 { return Some(String::new()); }
        Some(self.0.get(pos + 1..pos + 1 + n as usize)?.iter().map(|&c| c as char).collect())
    }
}

pub const PAGE_TRACKS: u32 = 0;
pub const PAGE_GENRES: u32 = 1;
pub const PAGE_ARTISTS: u32 = 2;
pub const PAGE_ALBUMS: u32 = 3;
pub const PAGE_LABELS: u32 = 4;
pub const PAGE_KEYS: u32 = 5;
pub const PAGE_PLAYLIST_TREE: u32 = 7;
pub const PAGE_PLAYLIST_ENTRIES: u32 = 8;

/// every present row of one table: (row offset in the file)
fn pdb_rows(d: &Rd, ty: u32) -> Vec<usize> {
    let mut out = vec![];
    let (Some(len_page), Some(num_tables)) = (d.u32le(4), d.u32le(8)) else { return out };
    let len_page = len_page as usize;
    if len_page == 0 { return out; }
    for t in 0..num_tables as usize {
        let tp = 28 + t * 16;
        if d.u32le(tp) != Some(ty) { continue; }
        let (Some(mut page), Some(last)) = (d.u32le(tp + 8), d.u32le(tp + 12)) else { continue };
        let mut seen = std::collections::HashSet::new();
        while !seen.contains(&page) && (page as usize + 1) * len_page <= d.0.len() {
            seen.insert(page);
            let p = page as usize * len_page;
            if d.u32le(p + 8) != Some(ty) { break; }
            let packed = d.u8(p + 24).unwrap_or(0) as u32 | (d.u8(p + 25).unwrap_or(0) as u32) << 8 | (d.u8(p + 26).unwrap_or(0) as u32) << 16;
            let num_offsets = (packed & 0x1fff) as usize;
            let flags = d.u8(p + 27).unwrap_or(0);
            if flags & 0x40 == 0 && num_offsets > 0 {
                let groups = (num_offsets - 1) / 16 + 1;
                for g in 0..groups {
                    let base = p + len_page - g * 0x24;
                    let present = d.u16le(base - 4).unwrap_or(0);
                    for r in 0..16 {
                        if g * 16 + r >= num_offsets { break; }
                        if (present >> r) & 1 == 0 { continue; }
                        if let Some(ofs) = d.u16le(base - (6 + 2 * r)) { out.push(p + 40 + ofs as usize); }
                    }
                }
            }
            if page == last { break; }
            match d.u32le(p + 12) { Some(n) => page = n, None => break }
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct PdbTrack {
    pub id: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub label: String,
    pub key: String,
    pub bpm: f64,
    pub duration: u32,
    pub size: u32,
    pub sample_rate: u32,
    pub rating: u8,
    pub color: u8,
    pub path: String,
    pub filename: String,
    pub anlz_path: String,
    pub comment: String,
    pub date_added: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdbNode { pub parent: u32, pub sort: u32, pub id: u32, pub folder: bool, pub name: String, pub tracks: Vec<u32> }

#[derive(Clone, Debug, Default)]
pub struct Pdb { pub tracks: Vec<PdbTrack>, pub playlists: Vec<PdbNode> }

pub fn parse_pdb(buf: &[u8]) -> Result<Pdb, String> {
    let d = Rd(buf);
    if buf.len() < 40 || d.u32le(4).unwrap_or(0) == 0 { return Err("not a rekordbox export.pdb".into()); }
    let named = |ty: u32, id_at: usize, f: &dyn Fn(&Rd, usize) -> Option<String>| -> HashMap<u32, String> {
        let mut m = HashMap::new();
        for o in pdb_rows(&d, ty) { if let (Some(id), Some(s)) = (d.u32le(o + id_at), f(&d, o)) { m.insert(id, s); } }
        m
    };
    let near_far = |near: usize, far: usize| move |v: &Rd, o: usize| -> Option<String> {
        let sub = v.u16le(o)?;
        v.string(o + if sub & 0x04 != 0 { v.u16le(o + far)? as usize } else { v.u8(o + near)? as usize })
    };
    let artists = named(PAGE_ARTISTS, 4, &near_far(9, 0x0a));
    let albums = named(PAGE_ALBUMS, 12, &near_far(21, 0x16));
    let genres = named(PAGE_GENRES, 0, &|v, o| v.string(o + 4));
    let keys = named(PAGE_KEYS, 0, &|v, o| v.string(o + 8));
    let labels = named(PAGE_LABELS, 0, &|v, o| v.string(o + 4));
    let mut tracks = vec![];
    for o in pdb_rows(&d, PAGE_TRACKS) {
        let t = (|| -> Option<PdbTrack> {
            let s = |k: usize| -> Option<String> { d.string(o + d.u16le(o + 94 + 2 * k)? as usize) };
            let g = |m: &HashMap<u32, String>, at: usize| -> String { d.u32le(o + at).and_then(|id| m.get(&id).cloned()).unwrap_or_default() };
            Some(PdbTrack {
                id: d.u32le(o + 72)?, title: s(17)?, artist: g(&artists, 68), album: g(&albums, 64), genre: g(&genres, 60), label: g(&labels, 40), key: g(&keys, 32),
                bpm: d.u32le(o + 56)? as f64 / 100.0, duration: d.u16le(o + 84)? as u32, size: d.u32le(o + 16)?, sample_rate: d.u32le(o + 8)?,
                rating: d.u8(o + 89)?, color: d.u8(o + 88)?, path: s(20)?, filename: s(19)?, anlz_path: s(14)?, comment: s(16).unwrap_or_default(), date_added: s(10).unwrap_or_default(),
            })
        })();
        if let Some(t) = t { tracks.push(t); }
    }
    let mut tree = vec![];
    for o in pdb_rows(&d, PAGE_PLAYLIST_TREE) {
        if let (Some(parent), Some(sort), Some(id), Some(folder), Some(name)) = (d.u32le(o), d.u32le(o + 8), d.u32le(o + 12), d.u32le(o + 16), d.string(o + 20)) {
            tree.push(PdbNode { parent, sort, id, folder: folder != 0, name, tracks: vec![] });
        }
    }
    let mut by_list: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for o in pdb_rows(&d, PAGE_PLAYLIST_ENTRIES) {
        if let (Some(index), Some(track), Some(pl)) = (d.u32le(o), d.u32le(o + 4), d.u32le(o + 8)) { by_list.entry(pl).or_default().push((index, track)); }
    }
    for n in tree.iter_mut() {
        if !n.folder { let mut e = by_list.remove(&n.id).unwrap_or_default(); e.sort(); n.tracks = e.into_iter().map(|x| x.1).collect(); }
    }
    tree.sort_by_key(|n| (n.parent, n.sort));
    Ok(Pdb { tracks, playlists: tree })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat { pub n: u16, pub bpm: f64, pub ms: u32 }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhraseList { pub mood: u16, pub end_beat: u16, pub list: Vec<(u16, u16)> }
#[derive(Clone, Debug, Default)]
pub struct Anlz { pub beats: Vec<Beat>, pub hot: Vec<RbCue>, pub memory: Vec<RbCue>, pub phrases: Option<PhraseList> }

/// ANLZ (.DAT/.EXT): beat grid, hot and memory cues, rekordbox's own phrase analysis
pub fn parse_anlz(buf: &[u8]) -> Anlz {
    let d = Rd(buf);
    let mut out = Anlz::default();
    if buf.len() < 12 || d.u32be(0) != Some(0x504d4149) { return out; } // "PMAI"
    let Some(mut pos) = d.u32be(4).map(|v| v as usize) else { return out };
    let tag = |s: &[u8; 4]| u32::from_be_bytes(*s);
    let (pqtz, pcob, pco2, pssi) = (tag(b"PQTZ"), tag(b"PCOB"), tag(b"PCO2"), tag(b"PSSI"));
    let mut cues2 = RbCues::default();
    while pos + 12 <= buf.len() {
        let (Some(four), Some(len_tag)) = (d.u32be(pos), d.u32be(pos + 8).map(|v| v as usize)) else { break };
        let b = pos + 12;
        if len_tag < 12 { break; }
        if four == pqtz {
            let n = d.u32be(b + 8).unwrap_or(0) as usize;
            for i in 0..n {
                let e = b + 12 + i * 8;
                match (d.u16be(e), d.u16be(e + 2), d.u32be(e + 4)) { (Some(n), Some(t), Some(ms)) => out.beats.push(Beat { n, bpm: t as f64 / 100.0, ms }), _ => break }
            }
        } else if four == pcob || four == pco2 {
            let is_ob = four == pcob;
            let n = d.u16be(b + if is_ob { 6 } else { 4 }).unwrap_or(0) as usize;
            let mut e = b + if is_ob { 12 } else { 8 };
            let mut i = 0;
            while i < n && e + 12 <= pos + len_tag {
                let (Some(len_entry), Some(hot)) = (d.u32be(e + 8), d.u32be(e + 12)) else { break };
                let typ = d.u8(e + if is_ob { 28 } else { 16 }).unwrap_or(0);
                let ms = d.u32be(e + if is_ob { 32 } else { 20 }).unwrap_or(0);
                let loop_ms = d.u32be(e + if is_ob { 36 } else { 24 }).unwrap_or(0);
                // (PCOB's status field only marks an active loop: 0 is an ordinary cue, so it's not a filter)
                let cue = RbCue { hot, ms, loop_ms: if typ == 2 { Some(loop_ms) } else { None } };
                let (hl, ml) = if four == pco2 { (&mut cues2.hot, &mut cues2.memory) } else { (&mut out.hot, &mut out.memory) };
                if hot != 0 { hl.push(cue); } else { ml.push(cue); }
                e += if len_entry > 0 { len_entry as usize } else { 56 };
                i += 1;
            }
        } else if four == pssi {
            let len_entries = d.u16be(b + 4).unwrap_or(0);
            let body_len = len_tag.saturating_sub(18);
            if let Some(src) = buf.get(b + 6..b + 6 + body_len) {
                let mut body = src.to_vec();
                if body.len() >= 2 && ((body[0] as u16) << 8 | body[1] as u16) > 20 {
                    const M: [u8; 19] = [0xcb, 0xe1, 0xee, 0xfa, 0xe5, 0xee, 0xad, 0xee, 0xe9, 0xd2, 0xe9, 0xeb, 0xe1, 0xe9, 0xf3, 0xe8, 0xe9, 0xf4, 0xe1];
                    for (i, x) in body.iter_mut().enumerate() { *x ^= M[i % M.len()].wrapping_add(len_entries as u8); }
                }
                let bv = Rd(&body);
                let mood = bv.u16be(0).unwrap_or(0);
                let end_beat = bv.u16be(8).unwrap_or(0);
                let mut list = vec![];
                for i in 0..len_entries as usize {
                    let e = 14 + i * 24;
                    if e + 6 > body.len() { break; }
                    list.push((bv.u16be(e + 2).unwrap(), bv.u16be(e + 4).unwrap()));
                }
                out.phrases = Some(PhraseList { mood, end_beat, list });
            }
        }
        pos += len_tag;
    }
    // the extended (.EXT) cue lists hold all eight hot cues: prefer them when present
    if !cues2.hot.is_empty() { out.hot = cues2.hot; }
    if !cues2.memory.is_empty() { out.memory = cues2.memory; }
    out
}

/// rekordbox phrase names -> Cratebox's
pub fn phrase_label(mood: u16, kind: u16) -> &'static str {
    if mood == 1 { return match kind { 1 => "Intro", 2 => "Build", 3 => "Breakdown", 5 => "Drop", 6 => "Outro", _ => "Groove" }; }
    match kind { 1 => "Intro", 8 => "Breakdown", 9 => "Drop", 10 => "Outro", _ => "Groove" }
}
/// constant-tempo grid: the most common tempo and the first downbeat
pub fn grid_from_beats(beats: &[Beat]) -> Option<(f64, f64)> {
    if beats.len() < 2 { return None; }
    let mut count: Vec<(u32, usize)> = vec![];
    for b in beats {
        let k = (b.bpm * 100.0).round() as u32;
        match count.iter_mut().find(|c| c.0 == k) { Some(c) => c.1 += 1, None => count.push((k, 1)) }
    }
    let best = count.iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0).reverse())).unwrap();
    let bpm = count.iter().find(|c| c.1 == best.1).unwrap().0 as f64 / 100.0; // first most-common, like the web version
    let down = beats.iter().find(|b| b.n == 1).unwrap_or(&beats[0]);
    Some((bpm, down.ms as f64 / 1000.0))
}
pub fn phrases_from(beats: &[Beat], p: &PhraseList) -> Option<Vec<Phrase>> {
    if p.list.is_empty() || beats.is_empty() { return None; }
    let at = |beat: u16| beats[(beat as usize).saturating_sub(1).min(beats.len() - 1)].ms as f64 / 1000.0;
    Some(p.list.iter().enumerate().map(|(i, &(beat, kind))| {
        let next = if i + 1 < p.list.len() { p.list[i + 1].0 } else if p.end_beat > 0 { p.end_beat } else { beat + 32 };
        Phrase { start: at(beat), bars: (((next as f64 - beat as f64) / 4.0).round() as i64).max(1) as u32, label: phrase_label(p.mood, kind).into() }
    }).collect())
}

// ---- rekordbox XML (File > Export Collection in xml format) ----
#[derive(Clone, Debug, Default)]
pub struct XmlMark { pub num: i32, pub start: f64, pub end: Option<f64>, pub kind: i32, pub name: String }
#[derive(Clone, Debug, Default)]
pub struct XmlTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub key: String,
    pub bpm: f64,
    pub duration: f64,
    pub size: u64,
    pub location: String,
    pub filename: String,
    /// (bpm, first beat, which beat of the bar it is)
    pub grid: Option<(f64, f64, u32)>,
    pub marks: Vec<XmlMark>,
}
#[derive(Clone, Debug, Default)]
pub struct XmlNode { pub name: String, pub folder: bool, pub parent: Option<usize>, pub tracks: Vec<String>, pub key: usize }
#[derive(Clone, Debug, Default)]
pub struct Xml { pub tracks: Vec<XmlTrack>, pub playlists: Vec<XmlNode> }

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
        }
        out.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn parse_xml(text: &str) -> Result<Xml, String> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(text);
    r.config_mut().trim_text(true);
    let mut out = Xml::default();
    let mut path: Vec<String> = vec![];
    let mut cur: Option<XmlTrack> = None;
    let mut node_stack: Vec<usize> = vec![]; // playlist NODEs open (indices into out.playlists; usize::MAX = ROOT)
    let mut saw_root = false;
    let attrs = |e: &quick_xml::events::BytesStart| -> HashMap<String, String> {
        e.attributes().flatten().map(|a| (String::from_utf8_lossy(a.key.as_ref()).to_string(), a.unescape_value().map(|v| v.to_string()).unwrap_or_default())).collect()
    };
    loop {
        let ev = r.read_event().map_err(|e| format!("not a rekordbox XML file ({e})"))?;
        let (start, empty) = match &ev { Event::Start(_) => (true, false), Event::Empty(_) => (true, true), _ => (false, false) };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) if start => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let a = attrs(e);
                let g = |k: &str| a.get(k).cloned().unwrap_or_default();
                let parent = path.last().map(|s| s.as_str()).unwrap_or("");
                match (name.as_str(), parent) {
                    ("DJ_PLAYLISTS", _) => saw_root = true,
                    ("TRACK", "COLLECTION") => {
                        let mut loc = g("Location");
                        if let Some(rest) = loc.strip_prefix("file://localhost") { loc = rest.to_string(); }
                        loc = url_decode(&loc);
                        let filename = loc.rsplit(['/', '\\']).next().unwrap_or("").to_string();
                        let t = XmlTrack { id: g("TrackID"), title: g("Name"), artist: g("Artist"), key: g("Tonality"), bpm: g("AverageBpm").parse().unwrap_or(0.0),
                            duration: g("TotalTime").parse().unwrap_or(0.0), size: g("Size").parse().unwrap_or(0), location: loc, filename, grid: None, marks: vec![] };
                        if empty { out.tracks.push(t); } else { cur = Some(t); }
                    }
                    ("TEMPO", "TRACK") => {
                        if let Some(t) = cur.as_mut() { if t.grid.is_none() { t.grid = Some((g("Bpm").parse().unwrap_or(0.0), g("Inizio").parse().unwrap_or(0.0), g("Battito").parse().unwrap_or(1))); } }
                    }
                    ("POSITION_MARK", "TRACK") => {
                        if let Some(t) = cur.as_mut() {
                            t.marks.push(XmlMark { num: g("Num").parse().unwrap_or(-1), start: g("Start").parse().unwrap_or(0.0), end: a.get("End").and_then(|v| v.parse().ok()),
                                kind: g("Type").parse().unwrap_or(0), name: g("Name") });
                        }
                    }
                    ("NODE", _) if path.iter().any(|p| p == "PLAYLISTS") => {
                        if node_stack.is_empty() && parent == "PLAYLISTS" {
                            node_stack.push(usize::MAX); // ROOT
                        } else {
                            let par = node_stack.last().copied().filter(|&p| p != usize::MAX);
                            let key = out.playlists.len();
                            out.playlists.push(XmlNode { name: g("Name"), folder: g("Type") == "0", parent: par, tracks: vec![], key });
                            if !empty { node_stack.push(key); }
                        }
                        if empty && node_stack.last() == Some(&usize::MAX) && parent == "PLAYLISTS" { node_stack.pop(); }
                    }
                    ("TRACK", "NODE") => {
                        if let Some(&k) = node_stack.last() { if k != usize::MAX { out.playlists[k].tracks.push(g("Key")); } }
                    }
                    _ => {}
                }
                if !empty { path.push(name); }
            }
            Event::End(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "TRACK" && path.len() >= 2 && path[path.len() - 2] == "COLLECTION" { if let Some(t) = cur.take() { out.tracks.push(t); } }
                if name == "NODE" && path.iter().any(|p| p == "PLAYLISTS") { node_stack.pop(); }
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_root { return Err("not a rekordbox XML file".into()); }
    Ok(out)
}

pub fn xml_esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }
/// a rekordbox-style Location: file://localhost/ + percent-encoded path
pub fn location_url(full: &str) -> String {
    let p = full.replace('\\', "/");
    let p = p.trim_start_matches('/');
    let mut s = String::from("file://localhost/");
    for b in p.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"/-_.~!$&'()*+,;=:@".contains(&b);
        if keep { s.push(b as char); } else { s.push_str(&format!("%{b:02X}")); }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(p: &str) -> Vec<u8> { std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rekordbox_usb").join(p)).unwrap() }
    #[test]
    fn real_usb_export() {
        let db = parse_pdb(&fixture("PIONEER/rekordbox/export.pdb")).unwrap();
        let mut titles: Vec<&str> = db.tracks.iter().map(|t| t.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, ["Demo Track 1", "Demo Track 2", "HORN", "NOISE", "SINEWAVE", "SIREN"]);
        let d1 = db.tracks.iter().find(|t| t.title == "Demo Track 1").unwrap();
        assert_eq!((d1.artist.as_str(), d1.key.as_str(), d1.bpm, d1.duration), ("Loopmasters", "Fm", 128.0, 172));
        assert_eq!(d1.path, "/Contents/Loopmasters/UnknownAlbum/Demo Track 1.mp3");
        assert_eq!(d1.anlz_path, "/PIONEER/USBANLZ/P016/0000875E/ANLZ0000.DAT");
        let pl: HashMap<&str, &PdbNode> = db.playlists.iter().map(|p| (p.name.as_str(), p)).collect();
        assert_eq!(pl["Playlist 1"].tracks, vec![1, 2]);
        assert!(pl["Folder"].folder && pl["Sub Playlist"].parent == pl["Folder"].id);
        let dat = parse_anlz(&fixture(&d1.anlz_path[1..]));
        let ext = parse_anlz(&fixture(&d1.anlz_path[1..].replace(".DAT", ".EXT")));
        assert_eq!(grid_from_beats(&dat.beats), Some((128.0, 0.025)));
        let ph = phrases_from(&dat.beats, ext.phrases.as_ref().unwrap()).unwrap();
        assert_eq!(ph.len(), 19);
        assert!((ph[0].start - 0.025).abs() < 1e-9);
    }
    #[test]
    fn hand_built_cue_lists() {
        // PCO2 hot list with hot cue A at 2 s and C at 9 s, memory list with one at 0.5 s
        let ent = |hot: u32, ms: u32| { let mut e = vec![0u8; 0x38]; e[..4].copy_from_slice(b"PCP2"); e[4..8].copy_from_slice(&0x10u32.to_be_bytes()); e[8..12].copy_from_slice(&0x38u32.to_be_bytes());
            e[12..16].copy_from_slice(&hot.to_be_bytes()); e[16] = 1; e[20..24].copy_from_slice(&ms.to_be_bytes()); e[24..28].copy_from_slice(&u32::MAX.to_be_bytes()); e };
        let list = |hot_list: bool, ents: Vec<Vec<u8>>| { let n = 20 + ents.iter().map(|e| e.len()).sum::<usize>(); let mut b = vec![0u8; 20];
            b[..4].copy_from_slice(b"PCO2"); b[4..8].copy_from_slice(&20u32.to_be_bytes()); b[8..12].copy_from_slice(&(n as u32).to_be_bytes());
            b[12..16].copy_from_slice(&(hot_list as u32).to_be_bytes()); b[16..18].copy_from_slice(&(ents.len() as u16).to_be_bytes()); for e in ents { b.extend(e); } b };
        let mut f = vec![0u8; 28];
        f[..4].copy_from_slice(b"PMAI"); f[4..8].copy_from_slice(&28u32.to_be_bytes());
        f.extend(list(true, vec![ent(1, 2000), ent(3, 9000)]));
        f.extend(list(false, vec![ent(0, 500)]));
        // and the older PCOB list (.DAT): status 0 entries are ordinary cues
        let pcpt = |hot: u32, ms: u32, status: u32| { let mut e = vec![0u8; 0x38]; e[..4].copy_from_slice(b"PCPT"); e[4..8].copy_from_slice(&0x1cu32.to_be_bytes());
            e[8..12].copy_from_slice(&0x38u32.to_be_bytes()); e[12..16].copy_from_slice(&hot.to_be_bytes()); e[16..20].copy_from_slice(&status.to_be_bytes());
            e[28] = 1; e[32..36].copy_from_slice(&ms.to_be_bytes()); e };
        let mut d = vec![0u8; 28];
        d[..4].copy_from_slice(b"PMAI"); d[4..8].copy_from_slice(&28u32.to_be_bytes());
        let body = [pcpt(0, 1234, 0), pcpt(2, 8000, 4)].concat();
        let mut sec = vec![0u8; 24];
        sec[..4].copy_from_slice(b"PCOB"); sec[4..8].copy_from_slice(&24u32.to_be_bytes()); sec[8..12].copy_from_slice(&((24 + body.len()) as u32).to_be_bytes());
        sec[18..20].copy_from_slice(&2u16.to_be_bytes());
        d.extend(sec); d.extend(body);
        let o = parse_anlz(&d);
        assert_eq!((o.memory.len(), o.memory[0].ms, o.hot.len(), o.hot[0].hot), (1, 1234, 1, 2));
        let a = parse_anlz(&f);
        assert_eq!(a.hot.iter().map(|c| (c.hot, c.ms)).collect::<Vec<_>>(), vec![(1, 2000), (3, 9000)]);
        assert_eq!(a.memory.iter().map(|c| c.ms).collect::<Vec<_>>(), vec![500]);
    }
    #[test]
    fn xml_round_trip_bits() {
        let x = r#"<?xml version="1.0" encoding="UTF-8"?><DJ_PLAYLISTS Version="1.0.0"><COLLECTION Entries="1">
<TRACK TrackID="11" Name="House &amp; Groove" Artist="A" Size="10" TotalTime="60" AverageBpm="126.00" Tonality="Am" Location="file://localhost/C:/Music/A%20-%20House.wav">
<TEMPO Inizio="0.367" Bpm="126.00" Metro="4/4" Battito="1"/><POSITION_MARK Name="" Type="0" Start="15.6" Num="0"/><POSITION_MARK Name="" Type="0" Start="0.367" Num="-1"/></TRACK>
<TRACK TrackID="12" Name="Ghost" Location="file://localhost/x/ghost.mp3"/></COLLECTION>
<PLAYLISTS><NODE Type="0" Name="ROOT" Count="2"><NODE Type="0" Name="Bass" Count="1"><NODE Name="Wobbles" Type="1" Entries="2"><TRACK Key="11"/><TRACK Key="12"/></NODE></NODE>
<NODE Name="Warmup" Type="1" Entries="1"><TRACK Key="11"/></NODE><NODE Name="Empty" Type="1" Entries="0"/></NODE></PLAYLISTS></DJ_PLAYLISTS>"#;
        let p = parse_xml(x).unwrap();
        assert_eq!(p.tracks.len(), 2);
        let t = &p.tracks[0];
        assert_eq!((t.title.as_str(), t.filename.as_str(), t.location.as_str()), ("House & Groove", "A - House.wav", "/C:/Music/A - House.wav"));
        assert_eq!(t.grid, Some((126.0, 0.367, 1)));
        assert_eq!(t.marks.len(), 2);
        let names: Vec<(&str, bool, Option<usize>, usize)> = p.playlists.iter().map(|n| (n.name.as_str(), n.folder, n.parent, n.tracks.len())).collect();
        assert_eq!(names, vec![("Bass", true, None, 0), ("Wobbles", false, Some(0), 2), ("Warmup", false, None, 1), ("Empty", false, None, 0)]);
        assert!(parse_xml("<foo/>").is_err());
        assert_eq!(location_url("C:\\Users\\jonah\\Music\\A - B #1.wav"), "file://localhost/C:/Users/jonah/Music/A%20-%20B%20%231.wav");
    }
}
