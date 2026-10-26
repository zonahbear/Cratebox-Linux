//! REC: the master (after the limiter, what the audience hears) to a 16-bit stereo WAV. The audio thread
//! pushes frames into a lock-free ring; a writer thread drains it to disk every 50 ms, so a slow disk
//! never holds up the sound (if it falls 8 s behind, frames are dropped and the file says so in the log).

use crate::ring::{ring, Consumer};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Recorder {
    pub path: PathBuf,
    pub started: Instant,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    sr: u32,
}

/// "~/Music/Cratebox Recordings" unless the settings say otherwise
pub fn default_dir() -> PathBuf {
    dirs::audio_dir().or_else(|| dirs::home_dir().map(|h| h.join("Music"))).unwrap_or_else(|| PathBuf::from(".")).join("Cratebox Recordings")
}

fn header(sr: u32, frames: u64) -> [u8; 44] {
    let data = (frames * 4).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&sr.to_le_bytes());
    h[28..32].copy_from_slice(&(sr * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

impl Recorder {
    /// start writing; returns the recorder and the ring's producer end for the engine (Cmd::Record)
    pub fn start(dir: &Path, sr: f64, name: &str) -> Result<(Recorder, crate::ring::Producer), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
        let path = dir.join(format!("{name}.wav"));
        let mut f = std::fs::File::create(&path).map_err(|e| format!("Couldn't create {}: {e}", path.display()))?;
        let sr = sr.round() as u32;
        f.write_all(&header(sr, 0)).map_err(|e| e.to_string())?;
        let (tx, rx) = ring(sr as usize * 8);
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));
        let (st, fr, er) = (stop.clone(), frames.clone(), error.clone());
        std::thread::Builder::new().name("cratebox-rec".into()).spawn(move || writer(f, rx, sr, st, fr, er)).map_err(|e| e.to_string())?;
        Ok((Recorder { path, started: Instant::now(), stop, frames, error, sr }, tx))
    }
    pub fn seconds(&self) -> f64 { self.frames.load(Ordering::Relaxed) as f64 / self.sr as f64 }
    pub fn take_error(&mut self) -> Option<String> { self.error.lock().ok().and_then(|mut e| e.take()) }
    /// the engine has been told to stop feeding it: finish the file
    pub fn stop(self) -> PathBuf { self.stop.store(true, Ordering::SeqCst); self.path }
}

fn writer(mut f: std::fs::File, mut rx: Consumer, sr: u32, stop: Arc<AtomicBool>, frames: Arc<AtomicU64>, error: Arc<Mutex<Option<String>>>) {
    let mut buf: Vec<u8> = Vec::with_capacity(sr as usize);
    let mut total = 0u64;
    let mut failed = false;
    let mut stopping: Option<Instant> = None;
    loop {
        buf.clear();
        rx.drain(|l, r| {
            for v in [l, r] { buf.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()); }
        });
        if !buf.is_empty() && !failed {
            match f.write_all(&buf) {
                Ok(()) => { total += buf.len() as u64 / 4; frames.store(total, Ordering::Relaxed); }
                Err(e) => { failed = true; if let Ok(mut x) = error.lock() { *x = Some(format!("Recording stopped: {e}")); } }
            }
        }
        if stop.load(Ordering::SeqCst) {
            // keep our end of the ring a little longer, so the audio thread never frees it
            let t = *stopping.get_or_insert_with(Instant::now);
            if t.elapsed() > Duration::from_millis(500) { break; }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&header(sr, total))).and_then(|_| f.sync_all());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_a_wav() {
        let dir = std::env::temp_dir().join(format!("cratebox-rec-{}", std::process::id()));
        let (r, mut p) = Recorder::start(&dir, 48000.0, "t").unwrap();
        for k in 0..4800 { p.push2((k as f32 / 4800.0) - 0.5, 0.25); }
        std::thread::sleep(Duration::from_millis(200));
        assert!((r.seconds() - 0.1).abs() < 1e-9);
        let path = r.stop();
        std::thread::sleep(Duration::from_millis(800));
        let b = std::fs::read(&path).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 4800 * 4);
        assert_eq!(b.len(), 44 + 4800 * 4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
