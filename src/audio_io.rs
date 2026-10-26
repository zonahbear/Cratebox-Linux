//! Sound output through cpal (ALSA on Linux, which also reaches PipeWire and PulseAudio through their
//! ALSA plugins; WASAPI shared mode on Windows). The engine lives behind a mutex that only the audio callback normally touches; it
//! try-locks, so switching devices never makes the callback wait. With no usable sound card the
//! engine still runs on a timer ("no output"), so the app keeps working.

use crate::engine::Engine;
use crate::ring::{ring, Consumer};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub type SharedEngine = Arc<Mutex<Engine>>;

#[derive(Clone, Debug)]
pub struct OutDev { pub name: String, pub max_channels: u16, pub rates: Vec<(u32, u32)> }

pub fn list_outputs() -> Vec<OutDev> {
    let host = cpal::default_host();
    let Ok(devs) = host.output_devices() else { return vec![] };
    let mut out = vec![];
    for d in devs {
        let Ok(name) = d.name() else { continue };
        let cfgs: Vec<_> = d.supported_output_configs().map(|c| c.collect()).unwrap_or_default();
        if cfgs.is_empty() { continue; }
        let max_channels = cfgs.iter().map(|c: &cpal::SupportedStreamConfigRange| c.channels()).max().unwrap_or(0);
        let rates = cfgs.iter().map(|c| (c.min_sample_rate().0, c.max_sample_rate().0)).collect();
        out.push(OutDev { name, max_channels, rates });
    }
    out
}

/// the FLX4 (or another Pioneer unit) if it's plugged in: its raw ALSA device, which has all 4 channels
pub fn guess_controller_output(devs: &[OutDev]) -> Option<String> {
    let pio = |n: &str| { let u = n.to_uppercase().replace('-', ""); u.contains("FLX4") || u.contains("DDJ") || u.contains("PIONEER") };
    devs.iter().filter(|d| pio(&d.name)).max_by_key(|d| (d.max_channels >= 4, d.name.starts_with("hw:"), d.name.starts_with("front:"))).map(|d| d.name.clone())
}

fn find_device(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if name.is_empty() || name == "default" { return host.default_output_device(); }
    host.output_devices().ok()?.find(|d| d.name().map(|n| n == name).unwrap_or(false))
}

/// pick a stream config at `rate` (or, with `rate` None, 48 kHz / 44.1 kHz / the device default),
/// with 4 channels when the device has them (master 1-2, headphones 3-4)
fn pick_config(dev: &cpal::Device, rate: Option<u32>, want_ch: u16, buffer: u32) -> Result<(StreamConfig, SampleFormat), String> {
    let cfgs: Vec<cpal::SupportedStreamConfigRange> = dev.supported_output_configs().map_err(|e| e.to_string())?.collect();
    let fmt_ok = |f: SampleFormat| matches!(f, SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32);
    let rates: Vec<u32> = match rate { Some(r) => vec![r], None => vec![48000, 44100] };
    for r in rates {
        let mut cands: Vec<&cpal::SupportedStreamConfigRange> = cfgs.iter().filter(|c| fmt_ok(c.sample_format()) && c.min_sample_rate().0 <= r && c.max_sample_rate().0 >= r).collect();
        // prefer the wanted channel count, then 2, then anything; f32 first
        cands.sort_by_key(|c| (c.channels() != want_ch, c.channels() != 2, c.channels(), c.sample_format() != SampleFormat::F32));
        if let Some(c) = cands.first() {
            let sc = (*c).with_sample_rate(SampleRate(r));
            let mut cfg: StreamConfig = sc.config();
            if buffer > 0 {
                if let cpal::SupportedBufferSize::Range { min, max } = sc.buffer_size() { cfg.buffer_size = cpal::BufferSize::Fixed(buffer.clamp(*min, *max)); }
            }
            return Ok((cfg, sc.sample_format()));
        }
    }
    if rate.is_some() { return Err(format!("that device can't run at {} Hz, the rate Cratebox started with. Restart Cratebox with it selected.", rate.unwrap())); }
    let d = dev.default_output_config().map_err(|e| e.to_string())?;
    if !fmt_ok(d.sample_format()) { return Err(format!("unsupported sample format {:?}", d.sample_format())); }
    Ok((d.config(), d.sample_format()))
}

fn build<T>(dev: &cpal::Device, cfg: &StreamConfig, mut fill: impl FnMut(&mut [f32]) + Send + 'static, err: Arc<Mutex<Option<String>>>, beat: Arc<AtomicU64>) -> Result<cpal::Stream, String>
where T: cpal::SizedSample + cpal::FromSample<f32> {
    let mut scratch: Vec<f32> = Vec::with_capacity(16384);
    let e2 = err.clone();
    let mut promoted = false;
    dev.build_output_stream::<T, _, _>(cfg, move |data: &mut [T], _| {
        if !promoted { promoted = true; promote_to_realtime(); }
        beat.fetch_add(1, Ordering::Relaxed);
        if scratch.len() < data.len() { scratch.resize(data.len(), 0.0); }
        let s = &mut scratch[..data.len()];
        fill(s);
        for (o, v) in data.iter_mut().zip(s.iter()) { *o = T::from_sample(*v); }
    }, move |e| { if let Ok(mut x) = e2.lock() { *x = Some(e.to_string()); } }, None).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn promote_to_realtime() { crate::win::pro_audio_thread(); }

#[cfg(unix)]
/// Puts the calling (audio callback) thread on SCHED_FIFO, so a busy UI or analysis thread can't delay
/// it. Needs an rtprio limit (Cratebox OS grants one); without it the thread stays at normal priority.
/// No RLIMIT_RTTIME is set (rtkit needs one, and the kernel kills the whole app with SIGXCPU when an
/// overloaded callback goes over it); the kernel's RT throttling still keeps the system usable.
fn promote_to_realtime() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let param = libc::sched_param { sched_priority: 70 };
    let rc = unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
    if rc != 0 && !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("cratebox: no realtime priority for the audio thread ({}), running at normal priority", std::io::Error::from_raw_os_error(rc));
    }
}

fn open_stream(dev: &cpal::Device, cfg: &StreamConfig, fmt: SampleFormat, fill: impl FnMut(&mut [f32]) + Send + 'static, err: Arc<Mutex<Option<String>>>, beat: Arc<AtomicU64>) -> Result<cpal::Stream, String> {
    let s = match fmt {
        SampleFormat::F32 => build::<f32>(dev, cfg, fill, err, beat)?,
        SampleFormat::I16 => build::<i16>(dev, cfg, fill, err, beat)?,
        SampleFormat::I32 => build::<i32>(dev, cfg, fill, err, beat)?,
        f => return Err(format!("unsupported sample format {f:?}")),
    };
    s.play().map_err(|e| e.to_string())?;
    Ok(s)
}

/// Counts a stream's callbacks so the UI thread can tell when it has stopped being called (after a
/// sleep/wake the PipeWire connection can die without the stream ever reporting an error).
struct Beat { n: Arc<AtomicU64>, seen: u64, at: Instant }

impl Beat {
    fn new() -> Beat { Beat { n: Arc::new(AtomicU64::new(0)), seen: 0, at: Instant::now() } }
    fn reset(&mut self) { self.seen = self.n.load(Ordering::Relaxed); self.at = Instant::now(); }
    /// true once the count hasn't moved for `limit`
    fn stale(&mut self, limit: Duration) -> bool {
        let n = self.n.load(Ordering::Relaxed);
        if n != self.seen { self.seen = n; self.at = Instant::now(); return false; }
        self.at.elapsed() > limit
    }
}

pub struct AudioIo {
    pub engine: SharedEngine,
    pub sr: f64,
    pub channels: usize,
    pub device: String,
    stream: Option<cpal::Stream>,
    hp_stream: Option<cpal::Stream>,
    pub hp_device: String,
    null_stop: Arc<AtomicBool>,
    null_thread: Option<JoinHandle<()>>,
    pub error: Arc<Mutex<Option<String>>>,
    beat: Beat,
    hp_beat: Beat,
}

fn render_into(engine: &SharedEngine, out: &mut [f32], ch: usize) {
    match engine.try_lock() {
        Ok(mut e) => e.render(out, ch),
        Err(_) => out.fill(0.0),
    }
}

impl AudioIo {
    /// pick the sample rate before the engine exists: the device's 48 kHz / 44.1 kHz / default
    pub fn probe_rate(name: &str) -> u32 {
        if let Some(d) = find_device(name) {
            if let Ok((c, _)) = pick_config(&d, None, 4, 0) { return c.sample_rate.0; }
        }
        48000
    }
    pub fn new(engine: SharedEngine, sr: f64) -> AudioIo {
        AudioIo { engine, sr, channels: 2, device: String::new(), stream: None, hp_stream: None, hp_device: String::new(), null_stop: Arc::new(AtomicBool::new(true)), null_thread: None, error: Arc::new(Mutex::new(None)), beat: Beat::new(), hp_beat: Beat::new() }
    }
    pub fn running(&self) -> bool { self.stream.is_some() }

    /// open `name` ("" = the system default); on failure the engine keeps running silently
    pub fn open(&mut self, name: &str, buffer: u32) -> Result<(), String> {
        self.stream = None;
        self.stop_null();
        if let Ok(mut e) = self.error.lock() { *e = None; }
        let res = (|| {
            let dev = find_device(name).ok_or_else(|| if name.is_empty() { "no sound card found".to_string() } else { format!("\"{name}\" isn't available") })?;
            let (cfg, fmt) = pick_config(&dev, Some(self.sr as u32), 4, buffer)?;
            let ch = cfg.channels as usize;
            let eng = self.engine.clone();
            let s = open_stream(&dev, &cfg, fmt, move |out| render_into(&eng, out, ch), self.error.clone(), self.beat.n.clone())?;
            Ok::<_, String>((s, ch))
        })();
        match res {
            Ok((s, ch)) => { self.stream = Some(s); self.beat.reset(); self.channels = ch; self.device = name.to_string(); Ok(()) }
            Err(e) => { self.start_null(); self.device = String::new(); Err(e) }
        }
    }
    /// no sound card: keep the decks moving on a timer, output discarded
    fn start_null(&mut self) {
        self.stop_null();
        self.channels = 2;
        let stop = Arc::new(AtomicBool::new(false));
        self.null_stop = stop.clone();
        let eng = self.engine.clone();
        let sr = self.sr;
        self.null_thread = Some(std::thread::Builder::new().name("cratebox-null-audio".into()).spawn(move || {
            let frames = 256usize;
            let mut buf = vec![0f32; frames * 2];
            let t0 = Instant::now();
            let mut done = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let due = (t0.elapsed().as_secs_f64() * sr) as u64;
                while done + frames as u64 <= due { render_into(&eng, &mut buf, 2); done += frames as u64; }
                std::thread::sleep(Duration::from_millis(2));
            }
        }).expect("thread"));
    }
    fn stop_null(&mut self) {
        self.null_stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.null_thread.take() { let _ = t.join(); }
    }
    /// a second device for the headphones (cue mix); None stops it
    pub fn open_headphones(&mut self, name: Option<&str>) -> Result<(), String> {
        self.hp_stream = None;
        if let Ok(mut e) = self.engine.lock() { e.hp_tap = None; }
        let Some(name) = name else { self.hp_device.clear(); return Ok(()) };
        let dev = find_device(name).ok_or_else(|| format!("\"{name}\" isn't available"))?;
        let (cfg, fmt) = pick_config(&dev, Some(self.sr as u32), 2, 0)?;
        let ch = cfg.channels as usize;
        let (prod, mut cons): (_, Consumer) = ring((self.sr * 0.5) as usize);
        let s = open_stream(&dev, &cfg, fmt, move |out| cons.pull(out, ch), self.error.clone(), self.hp_beat.n.clone())?;
        if let Ok(mut e) = self.engine.lock() { e.hp_tap = Some(prod); }
        self.hp_stream = Some(s);
        self.hp_beat.reset();
        self.hp_device = name.to_string();
        Ok(())
    }
    pub fn take_error(&self) -> Option<String> { self.error.lock().ok().and_then(|mut e| e.take()) }
    /// an open stream whose callbacks stopped for over a second (dead PipeWire connection after a
    /// wake, device gone): the caller closes and reopens. Suspend itself doesn't count, the
    /// monotonic clock stops while the machine sleeps.
    pub fn stalled(&mut self) -> bool {
        let limit = Duration::from_secs(1);
        (self.stream.is_some() && self.beat.stale(limit)) | (self.hp_stream.is_some() && self.hp_beat.stale(limit))
    }
    /// drop both streams and keep the decks moving on the timer until a reopen works
    pub fn close(&mut self) {
        self.hp_stream = None;
        if let Ok(mut e) = self.engine.lock() { e.hp_tap = None; }
        self.stream = None;
        self.start_null();
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) { self.hp_stream = None; self.stream = None; self.stop_null(); }
}
