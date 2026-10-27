//! Deck logic on the UI thread: transport, CDJ-style cue, sync (any of four decks can lead; half/double
//! time; beat lock), quantize, loops, hot cues, beat jump, jog handling, master tempo and the beat-grid
//! editor. Sync is a port of the iOS app's DeckModel (itself the web app's Deck class). Talks to the audio thread's player
//! through commands; side effects for the rest of the app (saving, messages) go out as events.

use crate::analysis::{Phrase, Wave, ANALYSIS_VER};
use crate::engine::Cmd;
use crate::keys::shift_key;
use crate::library::{Cues, Track};
use crate::player::{PlayerCmd, Report};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const RANGES: [f64; 4] = [6.0, 10.0, 16.0, 100.0];
pub const LOOP_SIZES: [f64; 8] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
pub const JUMPS: [f64; 8] = [-1.0, 1.0, -2.0, 2.0, -4.0, 4.0, -8.0, 8.0];
pub const TICK_SEC: f64 = 1.8 / 720.0; // 33 1/3 RPM platter, 720 ticks per revolution

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadMode { HotCue, BeatLoop, BeatJump, PadFx, PadFx2 }
impl PadMode {
    /// the FX bank (0 = FX1, 1 = FX2) for the PAD FX modes
    pub fn fx_bank(self) -> Option<usize> { match self { PadMode::PadFx => Some(0), PadMode::PadFx2 => Some(1), _ => None } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JogKind { Scratch, Bend, Search }

/// waveform columns squeezed to 1600 for the whole-track overview
#[derive(Clone, Debug, Default)]
pub struct Overview { pub peak: Vec<u8>, pub rms: Vec<u8>, pub low: Vec<u8>, pub mid: Vec<u8>, pub high: Vec<u8> }

pub fn build_overview(w: &Wave) -> Overview {
    const W: usize = 1600;
    let n = w.peak.len();
    let mut o = Overview { peak: vec![0; W], rms: vec![0; W], low: vec![0; W], mid: vec![0; W], high: vec![0; W] };
    if n == 0 { return o; }
    for x in 0..W {
        let a = x * n / W;
        let b = ((x + 1) * n / W).max(a + 1).min(n);
        let mx = |arr: &Vec<u8>| arr[a..b].iter().copied().max().unwrap_or(0);
        o.peak[x] = mx(&w.peak); o.rms[x] = mx(&w.rms); o.low[x] = mx(&w.low); o.mid[x] = mx(&w.mid); o.high[x] = mx(&w.high);
    }
    o
}

#[derive(Clone, Debug)]
pub struct DeckAna {
    pub bpm: f64,
    pub first_beat: f64,
    pub key: i32,
    pub phrases: Vec<Phrase>,
    pub wave: Arc<Wave>,
    pub overview: Arc<Overview>,
    pub duration: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Loop { pub on: bool, pub a: Option<f64>, pub b: Option<f64> }

pub enum DeckEvent {
    Status(String, f32),
    SaveCues(String, Cues),
    /// the track's analysis changed (grid edit, load): write it to the library + cache
    SaveTrack(Track),
    /// redo the phrases around an edited grid, in the background
    Rephrase { deck: usize, mono: Arc<Vec<f32>>, fs: f64, bpm: f64, first_beat: f64, gen: u64 },
    /// re-run the full analysis (grid reset with nothing to go back to)
    Reanalyse(String),
    Redraw,
}

pub struct Deck {
    pub i: usize,
    tx: Sender<Cmd>,
    pub track: Option<Track>,
    pub ana: Option<DeckAna>,
    pub duration: f64,
    pub sr: f64,
    pos: f64,
    pos_t: Instant,
    /// the waveform's own clock: follows `now()` smoothly instead of jumping with each audio report
    vis: std::cell::Cell<(f64, Instant)>,
    /// how far the drawn position sits behind `now()` (output latency + Settings offset - a frame), seconds
    pub vis_delay: f64,
    speed: f64,
    pub playing: bool,
    /// -1..1 from the tempo slider (top = negative, Pioneer style)
    pub pitch: f64,
    pub range_idx: usize,
    pub sync: bool,
    pub sync_since: Instant,
    pub sync_rate: f64,
    /// 0.5 / 1 / 2: half or double time against the master
    pub sync_mult: f64,
    /// the beat lock's tiny speed correction
    sync_trim: f64,
    /// the deck we follow while playing, and the track (load generation) it had when we locked on
    sync_with: Option<usize>,
    sync_track: u64,
    /// how far our beats are from the master's (seconds), while beat-locked
    pub phase_err: Option<f64>,
    /// whether there's a deck to follow right now (else a synced deck leads at its own tempo)
    pub has_master: bool,
    pub cue: f64,
    pub hot: [Option<f64>; 8],
    pub lp: Loop,
    pub quant: bool,
    pub pad_mode: PadMode,
    pub previewing: bool,
    pub touch: bool,
    pub loading: bool,
    pub loop_in_down: Option<Instant>,
    sent_rate: Option<f64>,
    pub coasting: bool,
    coast_hold: Instant,
    pub started_at: Option<Instant>,
    pub mt: bool,
    /// LOAD LOCK: nothing loads onto this deck until it's unlocked
    pub locked: bool,
    seek_seq: u64,
    taps: Vec<Instant>,
    pub mono: Option<(Arc<Vec<f32>>, f64)>,
    pub rephrase_gen: u64,
    rephrase_at: Option<Instant>,
    pub events: Vec<DeckEvent>,
    pub spin_secs: f64,
    pub load_gen: u64,
}

impl Deck {
    pub fn new(i: usize, tx: Sender<Cmd>) -> Deck {
        let now = Instant::now();
        Deck {
            i, tx, track: None, ana: None, duration: 0.0, sr: 44100.0, pos: 0.0, pos_t: now, vis: std::cell::Cell::new((0.0, now)), vis_delay: 0.0, speed: 0.0, playing: false, pitch: 0.0, range_idx: 0,
            sync: false, sync_since: now, sync_rate: 1.0, sync_mult: 1.0, sync_trim: 0.0, sync_with: None, sync_track: 0, phase_err: None, has_master: false, cue: 0.0, hot: [None; 8], lp: Loop::default(), quant: true,
            pad_mode: PadMode::HotCue, previewing: false, touch: false, loading: false, loop_in_down: None, sent_rate: None, coasting: false,
            coast_hold: now, started_at: None, mt: true, locked: false, seek_seq: 0, taps: vec![], mono: None, rephrase_gen: 0, rephrase_at: None,
            events: vec![], spin_secs: 2.5, load_gen: 0,
        }
    }
    fn post(&self, c: PlayerCmd) { let _ = self.tx.send(Cmd::Deck(self.i, c)); }
    fn say(&mut self, s: String) { self.events.push(DeckEvent::Status(s, 4.0)); }

    pub fn loaded(&self) -> bool { self.track.is_some() && !self.loading && self.duration > 0.0 }
    pub fn range(&self) -> f64 { RANGES[self.range_idx] }
    pub fn bpm(&self) -> f64 { self.ana.as_ref().map(|a| a.bpm).unwrap_or(0.0) }
    pub fn spb(&self) -> f64 { let b = self.bpm(); if b > 0.0 { 60.0 / b } else { 0.5 } }
    pub fn rate(&self) -> f64 {
        // synced with nothing to follow: holds its tempo
        if self.sync && self.bpm() > 0.0 { return self.sync_rate * (1.0 + self.sync_trim); }
        1.0 + self.pitch * self.range() / 100.0
    }
    pub fn eff_bpm(&self) -> f64 { self.bpm() * self.rate() / if self.sync { 1.0 + self.sync_trim } else { 1.0 } }
    /// what the other decks need to know about this one (for sync)
    pub fn peer(&self, active: bool) -> Peer {
        Peer { i: self.i, active, loaded: self.loaded(), bpm: self.bpm(), eff_bpm: self.eff_bpm(), beat: self.beat_at(self.now()), playing: self.playing,
            sync: self.sync, sync_since: self.sync_since, gen: self.load_gen, touch: self.touch, coasting: self.coasting, previewing: self.previewing, started_at: self.started_at }
    }
    /// with master tempo off, speeding up raises the key: 6% is about a semitone
    pub fn key_now(&self) -> i32 {
        match &self.ana {
            Some(a) if a.key >= 0 => if self.mt { a.key } else { shift_key(a.key, (12.0 * self.rate().log2()).round() as i32) },
            _ => -1,
        }
    }
    pub fn on_report(&mut self, r: &Report, at: Instant) {
        // a report from before our latest seek would undo it: skip those
        if r.seq < self.seek_seq { return; }
        self.pos = r.pos; self.pos_t = at; self.speed = r.speed;
        if Instant::now() > self.coast_hold { self.coasting = r.coast; }
        if self.playing && !r.playing && r.ended { self.playing = false; }
    }
    pub fn now(&self) -> f64 {
        let t = self.pos / self.sr + self.pos_t.elapsed().as_secs_f64() * self.speed;
        t.clamp(0.0, self.duration.max(0.0))
    }
    /// the playing position for drawing. Audio reports arrive once per output buffer, and their timing
    /// wobbles with the sound server's, so `now()` moves in small steps; drawn as is, the waveform
    /// judders. This clock runs at the deck's speed and eases a little towards `now()` each frame;
    /// seeks and stops (more than 40 ms off) snap.
    pub fn vis_now(&self) -> f64 {
        // what's heard when this frame reaches the screen, not what was just rendered
        let raw = (self.now() - self.vis_delay * self.speed).clamp(0.0, self.duration.max(0.0));
        let (vt, va) = self.vis.get();
        let pred = vt + va.elapsed().as_secs_f64() * self.speed;
        let err = raw - pred;
        let out = if self.speed == 0.0 || err.abs() > 0.04 { raw } else { pred + err * 0.1 };
        self.vis.set((out, Instant::now()));
        out
    }
    pub fn beat_at(&self, t: f64) -> f64 { match &self.ana { Some(a) => (t - a.first_beat) / self.spb(), None => 0.0 } }
    pub fn time_of_beat(&self, b: f64) -> f64 { match &self.ana { Some(a) => a.first_beat + b * self.spb(), None => 0.0 } }
    pub fn snap(&self, t: f64) -> f64 { if self.quant && self.ana.is_some() { self.time_of_beat(self.beat_at(t).round()) } else { t } }
    pub fn send_rate(&mut self) {
        let r = self.rate();
        if self.sent_rate != Some(r) { self.sent_rate = Some(r); self.post(PlayerCmd::Rate(r)); }
    }
    pub fn force_rate(&mut self) { self.sent_rate = None; self.send_rate(); }

    pub fn set_mt(&mut self, on: bool) {
        self.mt = on;
        self.post(PlayerCmd::Mt(on));
        self.say(format!("Deck {}: master tempo {}", self.i + 1, if on { "on (tempo changes keep the key)" } else { "off (vinyl: tempo changes the pitch)" }));
    }

    /// empty the deck (shutdown, battery critical); a load still decoding is dropped when it lands
    pub fn unload(&mut self) {
        self.playing = false; self.previewing = false;
        self.post(PlayerCmd::Unload);
        self.track = None; self.ana = None; self.mono = None; self.duration = 0.0; self.loading = false;
        self.load_gen += 1;
    }

    // ---- loading: the app decodes and analyses off-thread, then hands the result here ----
    pub fn begin_load(&mut self, t: Track) -> Result<u64, String> {
        if self.locked { return Err(format!("Deck {} is locked. Tap the lock on its screen to unlock it.", self.i + 1)); }
        if self.playing { return Err(format!("Deck {} is playing. Stop it before loading.", self.i + 1)); }
        self.loading = true;
        // a new track: sync starts over (master and half/double time picked again), from the fader's tempo
        self.sync_with = None; self.sync_trim = 0.0; self.sync_rate = 1.0 + self.pitch * self.range() / 100.0;
        self.track = Some(t);
        self.ana = None; self.mono = None; self.duration = 0.0;
        self.post(PlayerCmd::Unload); // free the previous track before decoding the next one
        self.load_gen += 1;
        Ok(self.load_gen)
    }
    /// decoded audio + analysis arrived. `track` is the library's (possibly updated) copy.
    pub fn finish_load(&mut self, gen: u64, mut track: Track, l: Arc<Vec<f32>>, r: Arc<Vec<f32>>, sr: f64, ana: DeckAna, mono: (Arc<Vec<f32>>, f64), cues: Cues) {
        if gen != self.load_gen { return; }
        self.sr = sr;
        self.duration = ana.duration;
        self.post(PlayerCmd::Load { l, r, mt: self.mt, spin: self.spin_secs });
        self.pos = 0.0; self.pos_t = Instant::now(); self.speed = 0.0;
        self.lp = Loop::default();
        self.previewing = false;
        self.mono = Some(mono);
        track.bpm = Some(ana.bpm); track.duration = Some(ana.duration); track.first_beat = Some(ana.first_beat); track.key = Some(ana.key);
        track.phrases = Some(ana.phrases.clone()); track.ver = ANALYSIS_VER;
        self.events.push(DeckEvent::SaveTrack(track.clone()));
        let title = track.title.clone();
        self.track = Some(track);
        let mut hot = [None; 8];
        for (k, h) in cues.hot.iter().take(8).enumerate() { hot[k] = *h; }
        self.hot = hot;
        self.cue = cues.cue.unwrap_or(ana.first_beat);
        let bpm = ana.bpm;
        self.ana = Some(ana);
        self.loading = false;
        let c = self.cue;
        self.seek(c);
        self.force_rate();
        self.say(format!("Deck {}: {} · {:.2} BPM", self.i + 1, title, bpm));
    }
    pub fn fail_load(&mut self, gen: u64, msg: &str) {
        if gen != self.load_gen { return; }
        self.loading = false; self.track = None; self.ana = None; self.duration = 0.0;
        self.events.push(DeckEvent::Status(format!("Couldn't load that file: {msg}"), 8.0));
    }

    // ---- transport ----
    /// jump somewhere: with Q on while playing, land on the same beat phase so a mix stays locked
    pub fn seek_to(&mut self, t: f64) {
        if !self.loaded() { return; }
        let mut t = t;
        if self.playing && self.quant && self.bpm() > 0.0 { let now = self.now(); t = now + ((t - now) / self.spb()).round() * self.spb(); }
        self.seek(t);
    }
    pub fn seek(&mut self, t: f64) {
        if !self.loaded() && self.duration <= 0.0 { return; }
        let t = t.clamp(0.0, (self.duration - 0.01).max(0.0));
        self.seek_seq += 1;
        self.post(PlayerCmd::Seek { pos: t * self.sr, seq: self.seek_seq });
        self.pos = t * self.sr; self.pos_t = Instant::now();
    }
    /// `peers`: every deck (this one included), for sync
    pub fn set_playing(&mut self, on: bool, peers: &[Peer]) {
        if !self.loaded() { return; }
        if on && !self.playing && self.sync { self.sync_tick(peers); } // follow the master it would pick now, before it counts as playing
        if on && self.sync { if let Some(m) = self.master(peers) { if m.playing { self.align_phase(m); } } }
        if on && !self.playing { self.started_at = Some(Instant::now()); }
        self.playing = on;
        self.previewing = false;
        self.send_rate();
        self.post(PlayerCmd::Play(on));
        self.speed = if on { self.rate() } else { 0.0 };
        self.pos = self.now() * self.sr; self.pos_t = Instant::now();
    }
    pub fn toggle_play(&mut self, peers: &[Peer]) { let p = !self.playing; self.set_playing(p, peers); }

    /// CDJ-style CUE: while playing, return to the cue point and stop; while stopped, set it,
    /// or if already on it, play for as long as the button is held.
    pub fn cue_down(&mut self) {
        if !self.loaded() { return; }
        if self.playing {
            self.set_playing(false, &[]);
            let c = self.cue; self.seek(c);
        } else if (self.now() - self.cue).abs() < 0.02 {
            self.previewing = true;
            self.playing = true;
            self.send_rate();
            self.post(PlayerCmd::Play(true));
            self.speed = self.rate();
        } else {
            self.cue = self.snap(self.now());
            let c = self.cue; self.seek(c);
            self.save_cues();
        }
    }
    pub fn cue_up(&mut self) {
        if self.previewing {
            self.previewing = false;
            self.playing = false;
            self.post(PlayerCmd::Play(false));
            let c = self.cue; self.seek(c);
        }
    }
    fn save_cues(&mut self) {
        if let Some(t) = &self.track { self.events.push(DeckEvent::SaveCues(t.id.clone(), Cues { cue: Some(self.cue), hot: self.hot.to_vec() })); }
    }

    // ---- sync ----
    /// the deck to follow when synced. A playing deck only ever follows the playing deck it already follows:
    /// it never picks up a new master mid-mix (a track loaded on another deck, a deck pressed into play), and
    /// when its master stops or loads it keeps its tempo and leads (see sync_tick).
    pub fn master(&self, peers: &[Peer]) -> Option<Peer> {
        if self.sync && self.playing {
            let w = peers.iter().find(|p| Some(p.i) == self.sync_with)?;
            return (w.playing && w.gen == self.sync_track && self.followable(w)).then_some(*w);
        }
        self.pick_master(peers)
    }
    fn followable(&self, o: &Peer) -> bool {
        o.i != self.i && o.active && o.loaded && o.bpm > 0.0 && !(o.sync && self.sync && o.sync_since > self.sync_since)
    }
    /// Two decks: the other one, unless it synced after us (then we lead). A playing deck only takes a playing one.
    /// Four decks: prefer one that isn't synced itself, then one that's playing, then the one playing longest.
    fn pick_master(&self, peers: &[Peer]) -> Option<Peer> {
        let mut c: Vec<Peer> = peers.iter().filter(|o| self.followable(o) && (o.playing || !self.playing)).copied().collect();
        let score = |o: &Peer| (if o.sync { 0 } else { 4 }) + (if o.playing { 2 } else { 0 });
        c.sort_by(|a, b| score(b).cmp(&score(a)).then_with(|| match (a.started_at, b.started_at) {
            (Some(x), Some(y)) => x.cmp(&y), (Some(_), None) => std::cmp::Ordering::Less, (None, Some(_)) => std::cmp::Ordering::Greater, _ => std::cmp::Ordering::Equal,
        }).then(a.i.cmp(&b.i)));
        c.first().copied()
    }
    /// of 1/2x, 1x and 2x the master's tempo, the one nearest our own
    fn pick_mult(r: f64) -> f64 { [0.5, 1.0, 2.0].into_iter().fold(1.0, |best, k| if (r * k).ln().abs() < (r * best).ln().abs() { k } else { best }) }
    pub fn set_sync(&mut self, on: bool, peers: &[Peer]) {
        if on && (!self.loaded() || self.bpm() <= 0.0) { return; }
        self.sync = on;
        if on {
            self.sync_since = Instant::now();
            let m = self.pick_master(peers);
            self.sync_with = m.map(|m| m.i); self.sync_track = m.map(|m| m.gen).unwrap_or(0);
            self.has_master = m.is_some();
            self.sync_rate = 1.0 + self.pitch * self.range() / 100.0; // with nothing to follow: stay at the fader's tempo
            match m {
                None => self.say("Load a track on another deck to sync to".into()),
                Some(m) => self.follow(m),
            }
        } else { self.sync_trim = 0.0; self.phase_err = None; }
        self.send_rate();
    }
    /// lock onto `m`'s tempo (and phase, if both play)
    fn follow(&mut self, m: Peer) {
        self.sync_with = Some(m.i); self.sync_track = m.gen; self.has_master = true;
        let r = m.eff_bpm / self.bpm();
        self.sync_mult = Self::pick_mult(r);
        self.sync_rate = (r * self.sync_mult).clamp(0.5, 2.0);
        self.sync_trim = 0.0;
        self.send_rate();
        if self.playing && m.playing { self.align_phase(m); }
        if self.sync_mult != 1.0 {
            self.say(format!("Deck {} synced at {} time: {:.2} against {:.2}", self.i + 1, if self.sync_mult < 1.0 { "half" } else { "double" }, self.bpm() * self.sync_rate, m.eff_bpm));
        }
    }
    /// SYNC held on another deck made `m` the master: a synced deck follows it from now on
    pub fn follow_master(&mut self, m: Peer) {
        if !self.sync || m.i == self.i || !self.loaded() || self.bpm() <= 0.0 || m.bpm <= 0.0 { return; }
        self.sync_since = Instant::now();
        self.follow(m);
    }
    /// SYNC held here: this deck leads at its current tempo (it stays synced, following nobody)
    pub fn become_master(&mut self) {
        self.sync_with = None; self.has_master = false; self.sync_trim = 0.0; self.phase_err = None;
        self.sync_since = Instant::now().checked_sub(std::time::Duration::from_secs(86400)).unwrap_or(self.sync_since.min(Instant::now())); // older than every other sync: never follows them
        self.send_rate();
    }
    pub fn align_phase(&mut self, m: Peer) {
        if self.bpm() <= 0.0 || m.bpm <= 0.0 { return; }
        let mb = m.beat * self.sync_mult;
        let frac = mb - mb.floor();
        let my_b = self.beat_at(self.now());
        let mut target = my_b.floor() + frac;
        if target - my_b > 0.5 { target -= 1.0; }
        if my_b - target > 0.5 { target += 1.0; }
        let t = self.time_of_beat(target);
        self.seek(t);
    }
    /// keep following the master's tempo and phase (called every frame)
    pub fn sync_tick(&mut self, peers: &[Peer]) {
        let m = self.master(peers);
        self.has_master = m.is_some();
        if !self.sync || self.bpm() <= 0.0 { return; }
        let Some(m) = m else {
            // the master stopped, loaded a new track or went away: keep this tempo and lead from here on (the
            // oldest sync, so a deck that syncs now follows us, not the other way round)
            if self.sync_with.is_some() {
                self.sync_with = None;
                if self.playing {
                    let oldest = peers.iter().map(|p| p.sync_since).min().unwrap_or(self.sync_since);
                    self.sync_since = oldest.checked_sub(Duration::from_millis(1)).unwrap_or(oldest);
                }
            }
            if self.sync_trim != 0.0 { self.sync_trim = 0.0; self.send_rate(); }
            self.phase_err = None;
            return;
        };
        // a different master, or a new track on it: pick half/normal/double time again
        if Some(m.i) != self.sync_with || m.gen != self.sync_track { self.sync_mult = Self::pick_mult(m.eff_bpm / self.bpm()); }
        self.sync_with = Some(m.i); self.sync_track = m.gen;
        let r = (m.eff_bpm * self.sync_mult / self.bpm()).clamp(0.5, 2.0);
        if (r - self.sync_rate).abs() > 1e-7 { self.sync_rate = r; }
        self.send_rate();
        self.lock_phase(m);
    }
    /// Beat lock: while both decks play, measure how far our beats are from the master's and correct with a
    /// tiny speed change. A deliberate offset of more than a quarter beat is left alone.
    fn lock_phase(&mut self, m: Peer) {
        let live = self.playing && m.playing && !self.touch && !m.touch && !self.coasting && !m.coasting && !self.previewing && !m.previewing && self.loaded() && m.loaded;
        if !live { if self.sync_trim != 0.0 { self.sync_trim = 0.0; self.send_rate(); } self.phase_err = None; return; }
        let mut err = self.beat_at(self.now()) - m.beat * self.sync_mult;
        err -= (err + 0.5).floor();
        let err_sec = err * self.spb() / self.rate().max(0.05);
        self.phase_err = Some(err_sec);
        let want = if err.abs() < 0.25 && err_sec.abs() > 0.0015 { (-err_sec / 1.5).clamp(-0.006, 0.006) } else { 0.0 };
        let trim = self.sync_trim + (want - self.sync_trim) * 0.2;
        if (trim - self.sync_trim).abs() > 1e-6 || (want == 0.0 && self.sync_trim != 0.0) {
            self.sync_trim = if trim.abs() < 1e-5 { 0.0 } else { trim };
            self.send_rate();
        }
    }
    pub fn cycle_range(&mut self) {
        self.range_idx = (self.range_idx + 1) % RANGES.len();
        self.send_rate();
        let r = self.range();
        self.say(format!("Deck {} tempo range ±{}", self.i + 1, if r == 100.0 { "WIDE".to_string() } else { format!("{r}%") }));
    }
    pub fn set_pitch(&mut self, v: f64) {
        let p = v.clamp(-1.0, 1.0);
        // a synced deck that leads (nothing to follow) holds its own tempo: the fader moves it from there
        if self.sync && self.bpm() > 0.0 && !self.has_master { self.sync_rate = (self.sync_rate + (p - self.pitch) * self.range() / 100.0).clamp(0.5, 2.0); }
        self.pitch = p;
        self.send_rate();
    }

    // ---- jog ----
    pub fn jog_touch(&mut self, on: bool, no_coast: bool) {
        if !self.loaded() { return; }
        self.touch = on;
        // the jog keeps turning after a fling: ignore its untouched ticks until the coast ends
        if !on && !no_coast { self.coasting = true; self.coast_hold = Instant::now() + Duration::from_millis(120); } else { self.coasting = false; }
        self.post(PlayerCmd::Scratch { on, no_coast });
    }
    pub fn jog_ticks(&mut self, ticks: f64, kind: JogKind) {
        if !self.loaded() { return; }
        if kind == JogKind::Scratch && self.touch { self.post(PlayerCmd::Jog(ticks * TICK_SEC * self.sr)); }
        else if self.coasting { }
        else if kind == JogKind::Search { let t = self.now() + ticks * 0.08; self.seek(t); }
        else if self.playing { self.post(PlayerCmd::Bend(ticks * 0.004)); }
        else { let t = self.now() + ticks * TICK_SEC; self.seek(t); }
    }
    pub fn set_spin(&mut self, secs: f64) { self.spin_secs = secs; self.post(PlayerCmd::Spin(secs)); }

    // ---- loops ----
    fn send_loop(&mut self) {
        let l = self.lp;
        self.post(PlayerCmd::Loop { on: l.on && l.a.is_some() && l.b.is_some(), a: l.a.unwrap_or(0.0) * self.sr, b: l.b.unwrap_or(0.0) * self.sr });
    }
    pub fn loop_in(&mut self) { if !self.loaded() { return; } self.lp = Loop { on: false, a: Some(self.snap(self.now())), b: None }; self.send_loop(); }
    pub fn loop_out(&mut self) {
        let Some(a) = self.lp.a else { return };
        if !self.loaded() { return; }
        let b = self.snap(self.now());
        if b <= a + 0.01 { return; }
        self.lp.b = Some(b); self.lp.on = true; self.send_loop();
    }
    pub fn reloop_exit(&mut self) {
        let (Some(a), Some(b)) = (self.lp.a, self.lp.b) else { return };
        if self.lp.on { self.lp.on = false; self.send_loop(); }
        else { self.lp.on = true; self.send_loop(); let n = self.now(); if n > b || n < a { self.seek(a); } }
    }
    pub fn beat_loop(&mut self, beats: f64) {
        if !self.loaded() { return; }
        let len = beats * self.spb();
        if let (true, Some(a), Some(b)) = (self.lp.on, self.lp.a, self.lp.b) {
            if ((b - a) - len).abs() < 0.002 { self.lp.on = false; self.send_loop(); return; }
        }
        let t = self.now();
        let a = if self.quant && self.ana.is_some() { self.time_of_beat((self.beat_at(t) + 1e-6).floor()) } else { t };
        self.lp = Loop { on: true, a: Some(a), b: Some(a + len) };
        self.send_loop();
    }
    pub fn loop_scale(&mut self, f: f64) {
        let (Some(a), Some(b)) = (self.lp.a, self.lp.b) else { return };
        let len = (b - a) * f;
        if len < self.spb() / 32.0 || len > self.spb() * 512.0 { return; }
        self.lp.b = Some(a + len);
        self.send_loop();
    }
    pub fn loop_beats(&self) -> f64 { match (self.lp.a, self.lp.b) { (Some(a), Some(b)) => (b - a) / self.spb(), _ => 0.0 } }
    pub fn beat_jump(&mut self, beats: f64) {
        if !self.loaded() { return; }
        let d = beats * self.spb();
        if self.lp.on { if let (Some(a), Some(b)) = (self.lp.a, self.lp.b) { self.lp.a = Some(a + d); self.lp.b = Some(b + d); self.send_loop(); } }
        let t = self.now() + d;
        self.seek(t);
    }

    // ---- hot cues ----
    pub fn hotcue(&mut self, i: usize) {
        if !self.loaded() { return; }
        match self.hot[i] {
            None => { self.hot[i] = Some(self.snap(self.now())); self.save_cues(); }
            Some(c) => { self.seek(c); if !self.playing { self.cue = c; } }
        }
    }
    pub fn clear_hot(&mut self, i: usize) { self.hot[i] = None; self.save_cues(); }
    pub fn call_cue(&mut self, dir: i32) {
        if self.lp.on { return self.loop_scale(if dir > 0 { 2.0 } else { 0.5 }); }
        let t = self.now();
        let mut cues: Vec<f64> = self.hot.iter().flatten().copied().collect();
        cues.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let next = if dir > 0 { cues.iter().copied().find(|&c| c > t + 0.05) } else { cues.iter().rev().copied().find(|&c| c < t - 0.05) };
        if let Some(n) = next { self.seek(n); if !self.playing { self.cue = n; } }
    }
    pub fn pad_press(&mut self, p: usize, shift: bool) {
        match self.pad_mode {
            PadMode::HotCue => if shift { self.clear_hot(p) } else { self.hotcue(p) },
            PadMode::BeatLoop => self.beat_loop(LOOP_SIZES[p]),
            PadMode::BeatJump => self.beat_jump(JUMPS[p]),
            PadMode::PadFx | PadMode::PadFx2 => {} // the app runs PAD FX (it owns the FX units)
        }
    }

    // ---- beat grid editing: every edit keeps the beat under the playhead, and firstBeat on a bar line ----
    pub fn set_grid(&mut self, bpm: f64, first_beat: f64, why: &str) {
        if !(bpm > 20.0 && bpm < 400.0) { return; }
        let Some(ana) = self.ana.as_mut() else { return };
        if let Some(t) = self.track.as_mut() {
            if !t.grid_manual { t.auto_bpm = Some(ana.bpm); t.auto_first = Some(ana.first_beat); }
        }
        let bar = 4.0 * 60.0 / bpm;
        let fb = first_beat - (first_beat / bar).floor() * bar;
        ana.bpm = bpm; ana.first_beat = fb;
        if let Some(t) = self.track.as_mut() {
            t.bpm = Some(bpm); t.first_beat = Some(fb); t.grid_manual = true;
            self.events.push(DeckEvent::SaveTrack(t.clone()));
        }
        self.schedule_rephrase();
        self.force_rate();
        if !why.is_empty() { self.say(format!("Deck {} grid: {why} · {bpm:.2} BPM", self.i + 1)); }
    }
    pub fn set_bpm(&mut self, bpm: f64, why: &str) {
        if self.ana.is_none() { return; }
        let bpm = (bpm * 1000.0).round() / 1000.0;
        let t = self.now();
        let b = self.beat_at(t);
        self.set_grid(bpm, t - b * (60.0 / bpm), if why.is_empty() { "BPM set" } else { why });
    }
    pub fn nudge_grid(&mut self, sec: f64) {
        let Some(a) = &self.ana else { return };
        let (b, f) = (a.bpm, a.first_beat);
        self.set_grid(b, f + sec, &format!("moved {} {:.0} ms", if sec > 0.0 { "later" } else { "earlier" }, (sec * 1000.0).abs()));
    }
    pub fn downbeat_here(&mut self) { if let Some(a) = &self.ana { let b = a.bpm; let t = self.now(); self.set_grid(b, t, "bar 1 set at the playhead"); } }
    pub fn beat_here(&mut self) {
        let Some(a) = &self.ana else { return };
        let bpm = a.bpm;
        let t = self.now();
        let b = self.beat_at(t).round();
        let s = self.spb();
        self.set_grid(bpm, t - b * s, "beat set at the playhead");
    }
    pub fn tap(&mut self) {
        let now = Instant::now();
        if self.taps.last().map_or(true, |l| now.duration_since(*l) > Duration::from_secs(2)) { self.taps.clear(); }
        self.taps.push(now);
        if self.taps.len() > 16 { self.taps.remove(0); }
        let n = self.taps.len();
        if n < 4 { self.say(format!("Tap {n}… keep tapping on the beat")); return; }
        // least-squares slope through the taps: steadier than averaging the gaps
        let t0 = self.taps[0];
        let (mut sx, mut sy, mut sxx, mut sxy) = (0f64, 0f64, 0f64, 0f64);
        for (x, t) in self.taps.iter().enumerate() {
            let x = x as f64; let y = t.duration_since(t0).as_secs_f64() * 1000.0;
            sx += x; sy += y; sxx += x * x; sxy += x * y;
        }
        let nf = n as f64;
        let ms = (nf * sxy - sx * sy) / (nf * sxx - sx * sx);
        self.set_bpm((60000.0 / ms * 100.0).round() / 100.0, &format!("tapped ({n} taps)"));
    }
    fn schedule_rephrase(&mut self) { self.rephrase_at = Some(Instant::now() + Duration::from_millis(500)); }
    /// called every frame: phrases hang off the bar lines, so redo them once grid edits settle
    pub fn tick(&mut self) {
        if let Some(at) = self.rephrase_at {
            if Instant::now() >= at {
                self.rephrase_at = None;
                if let (Some((m, fs)), Some(a)) = (&self.mono, &self.ana) {
                    self.rephrase_gen += 1;
                    self.events.push(DeckEvent::Rephrase { deck: self.i, mono: m.clone(), fs: *fs, bpm: a.bpm, first_beat: a.first_beat, gen: self.rephrase_gen });
                }
            }
        }
    }
    pub fn rephrased(&mut self, gen: u64, phrases: Vec<Phrase>) {
        if gen != self.rephrase_gen { return; }
        if let Some(a) = self.ana.as_mut() { a.phrases = phrases.clone(); }
        if let Some(t) = self.track.as_mut() { t.phrases = Some(phrases); self.events.push(DeckEvent::SaveTrack(t.clone())); }
        self.events.push(DeckEvent::Redraw);
    }
    pub fn reset_grid(&mut self) {
        let Some(t) = self.track.as_mut() else { return };
        if self.ana.is_none() { return; }
        let ana = self.ana.as_mut().unwrap();
        if let (Some(b), Some(f)) = (t.auto_bpm, t.auto_first) { ana.bpm = b; ana.first_beat = f; t.bpm = Some(b); t.first_beat = Some(f); }
        t.grid_manual = false;
        t.grid_from = None;
        t.ver = if t.auto_bpm.is_some() { ANALYSIS_VER } else { 0 };
        let tc = t.clone();
        let requeue = t.auto_bpm.is_none();
        self.events.push(DeckEvent::SaveTrack(tc.clone()));
        if requeue { self.events.push(DeckEvent::Reanalyse(tc.id)); }
        self.schedule_rephrase();
        self.force_rate();
        self.say(format!("Deck {}: grid back to the analysis", self.i + 1));
    }

    /// phrases with their end times on the current grid: (start, end, bars, label, bar length)
    pub fn phrase_spans(&self) -> Vec<(f64, f64, u32, String, f64)> {
        let Some(a) = &self.ana else { return vec![] };
        if a.bpm <= 0.0 { return vec![]; }
        let bar = 240.0 / a.bpm;
        a.phrases.iter().map(|p| (p.start, p.start + p.bars as f64 * bar, p.bars, p.label.clone(), bar)).collect()
    }
}

/// what a synced deck needs to know about another deck
#[derive(Clone, Copy, Debug)]
pub struct Peer {
    pub i: usize,
    /// shown (decks 3 and 4 only in 4-deck mode)
    pub active: bool,
    pub loaded: bool,
    pub bpm: f64,
    pub eff_bpm: f64,
    /// beat position now
    pub beat: f64,
    pub playing: bool,
    pub sync: bool,
    pub sync_since: Instant,
    /// load generation: a new track on it counts as a new master
    pub gen: u64,
    pub touch: bool,
    pub coasting: bool,
    pub previewing: bool,
    pub started_at: Option<Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    fn loaded(i: usize, bpm: f64) -> (Deck, std::sync::mpsc::Receiver<Cmd>) {
        let (tx, rx) = channel();
        let mut d = Deck::new(i, tx);
        let g = d.begin_load(Track { id: format!("t{i}"), title: "T".into(), ..Default::default() }).unwrap();
        let w = Arc::new(Wave::default());
        let ana = DeckAna { bpm, first_beat: 0.5, key: 21, phrases: vec![], wave: w, overview: Arc::new(Overview::default()), duration: 200.0 };
        let pcm = Arc::new(vec![0f32; 10]);
        d.finish_load(g, Track { id: format!("t{i}"), ..Default::default() }, pcm.clone(), pcm, 44100.0, ana, (Arc::new(vec![]), 11025.0), Cues::empty());
        (d, rx)
    }
    #[test]
    fn cue_defaults_to_first_beat_and_quantized_hotcues() {
        let (mut d, _rx) = loaded(0, 120.0);
        assert!((d.cue - 0.5).abs() < 1e-9);
        d.seek(10.26);
        d.hotcue(0);
        assert!((d.hot[0].unwrap() - 10.5).abs() < 1e-9, "snapped to the beat: {:?}", d.hot[0]);
        d.quant = false;
        d.seek(3.33);
        d.hotcue(1);
        assert!((d.hot[1].unwrap() - 3.33).abs() < 1e-6);
    }
    #[test]
    fn tempo_and_sync() {
        let (mut a, _ra) = loaded(0, 126.0);
        let (mut b, _rb) = loaded(1, 140.0);
        a.set_pitch(1.0); // bottom of the fader... (the app maps the slider)
        assert!((a.rate() - 1.06).abs() < 1e-12);
        a.set_pitch(0.0);
        let peers = |a: &Deck, b: &Deck| [a.peer(true), b.peer(true)];
        b.set_sync(true, &peers(&a, &b));
        assert!((b.eff_bpm() - 126.0).abs() < 1e-9, "synced to {}", b.eff_bpm());
        a.set_pitch(0.5);
        b.sync_tick(&peers(&a, &b));
        assert!((b.eff_bpm() - a.eff_bpm()).abs() < 1e-9);
        // a 70 BPM track syncs at half time to a 140 one (one of its beats per two of the master's)
        let (mut c, _rc) = loaded(2, 70.0);
        let (h, _rh) = loaded(3, 140.0);
        c.set_sync(true, &[h.peer(true), c.peer(true)]);
        assert_eq!(c.sync_mult, 0.5);
        assert!((c.eff_bpm() - 70.0).abs() < 1e-9, "plays at its own tempo: {}", c.eff_bpm());
        // a hidden deck is never the master
        let (mut d, _rd) = loaded(0, 126.0);
        d.set_sync(true, &[h.peer(false), d.peer(true)]);
        assert!(!d.has_master);
    }
    #[test]
    fn grid_edit_keeps_the_beat_under_the_playhead() {
        let (mut d, _rx) = loaded(0, 126.0);
        d.seek(30.0);
        let b0 = d.beat_at(d.now());
        let ph0 = b0 - b0.floor();
        d.set_bpm(128.0, "");
        let b1 = d.beat_at(d.now());
        let ph1 = b1 - b1.floor();
        assert!((ph0 - ph1).abs() < 1e-6, "{ph0} {ph1}");
        assert!(d.track.as_ref().unwrap().grid_manual);
        assert_eq!(d.track.as_ref().unwrap().auto_bpm, Some(126.0));
        d.reset_grid();
        assert_eq!(d.bpm(), 126.0);
    }
    #[test]
    fn beat_loops_and_jumps() {
        let (mut d, _rx) = loaded(0, 120.0);
        d.seek(10.6);
        d.beat_loop(4.0);
        assert_eq!((d.lp.a, d.lp.b), (Some(10.5), Some(12.5)));
        d.loop_scale(0.5);
        assert_eq!(d.lp.b, Some(11.5));
        assert!((d.loop_beats() - 2.0).abs() < 1e-9);
        d.beat_loop(2.0); // same size again: off
        assert!(!d.lp.on);
        d.seek(20.0);
        d.beat_jump(-4.0);
        assert!((d.now() - 18.0).abs() < 1e-3);
    }
}
