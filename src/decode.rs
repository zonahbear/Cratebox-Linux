//! Decoding audio files to planar f32 (symphonia: MP3, FLAC, WAV, AIFF, AAC/M4A, OGG Vorbis...),
//! plus the tags symphonia finds (title/artist) for formats without ID3.

use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::Hint;

pub struct Decoded {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub sr: u32,
    pub title: Option<String>,
    pub artist: Option<String>,
}

impl Decoded {
    pub fn duration(&self) -> f64 { self.l.len() as f64 / self.sr as f64 }
}

pub fn decode(path: &Path) -> Result<Decoded, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) { hint.with_extension(ext); }
    let mut probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
        .map_err(|e| format!("not an audio file Cratebox can read ({e})"))?;
    let (mut title, mut artist) = (None, None);
    let mut take_tags = |rev: &symphonia::core::meta::MetadataRevision| {
        for t in rev.tags() {
            match t.std_key {
                Some(StandardTagKey::TrackTitle) => title = Some(t.value.to_string()),
                Some(StandardTagKey::Artist) => artist = Some(t.value.to_string()),
                _ => {}
            }
        }
    };
    if let Some(m) = probed.metadata.get() { if let Some(r) = m.current() { take_tags(r); } }
    let mut format = probed.format;
    if let Some(r) = format.metadata().current() { take_tags(r); }
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio track in that file")?;
    let tid = track.id;
    let mut dec = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| format!("unsupported codec ({e})"))?;
    let mut sr = track.codec_params.sample_rate.unwrap_or(44100);
    let cap = track.codec_params.n_frames.unwrap_or(0) as usize;
    let (mut l, mut r) = (Vec::with_capacity(cap), Vec::with_capacity(cap));
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    loop {
        let pkt = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => { if l.is_empty() { return Err(e.to_string()); } break; }
        };
        if pkt.track_id() != tid { continue; }
        let buf = match dec.decode(&pkt) {
            Ok(b) => b,
            Err(Error::DecodeError(_)) => continue, // a damaged frame: skip it
            Err(e) => { if l.is_empty() { return Err(e.to_string()); } break; }
        };
        let spec = *buf.spec();
        sr = spec.rate;
        let ch = spec.channels.count().max(1);
        let need = buf.capacity() as u64;
        if sbuf.as_ref().map_or(true, |s| s.capacity() < need as usize * ch) { sbuf = Some(SampleBuffer::new(need, spec)); }
        let sb = sbuf.as_mut().unwrap();
        sb.copy_interleaved_ref(buf);
        let s = sb.samples();
        for f in s.chunks_exact(ch) {
            l.push(f[0]);
            r.push(if ch > 1 { f[1] } else { f[0] });
        }
    }
    if l.is_empty() { return Err("no audio in that file".into()); }
    Ok(Decoded { l, r, sr, title: title.filter(|s| !s.trim().is_empty()), artist: artist.filter(|s| !s.trim().is_empty()) })
}

/// Resample to the output device's rate (like the browser's decodeAudioData does), windowed-sinc,
/// 32 taps, with the cutoff lowered when going down so nothing folds back.
pub fn resample(x: &[f32], from: f64, to: f64) -> Vec<f32> {
    if (from - to).abs() < 0.5 || x.is_empty() { return x.to_vec(); }
    const TAPS: i64 = 16; // each side
    const PHASES: usize = 512;
    let ratio = to / from;
    let cut = ratio.min(1.0) * 0.97; // cutoff, as a fraction of the input Nyquist
    // table of the kernel at PHASES fractional offsets
    let mut table = vec![0f32; PHASES * (2 * TAPS as usize)];
    for p in 0..PHASES {
        let frac = p as f64 / PHASES as f64;
        let mut sum = 0f64;
        let mut row = [0f64; 32];
        for k in 0..2 * TAPS {
            let t = (k - TAPS + 1) as f64 - frac;
            let sinc = if t.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * t * cut).sin() / (std::f64::consts::PI * t * cut) };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * t / (TAPS as f64 + 0.5)).cos(); // Hann
            row[k as usize] = sinc * w;
            sum += sinc * w;
        }
        for k in 0..2 * TAPS as usize { table[p * 32 + k] = (row[k] / sum) as f32; }
    }
    let n_out = ((x.len() as f64) * ratio).floor() as usize;
    let mut out = vec![0f32; n_out];
    let step = from / to;
    let n = x.len() as i64;
    for (i, o) in out.iter_mut().enumerate() {
        let pos = i as f64 * step;
        let i0 = pos.floor() as i64;
        let frac = pos - i0 as f64;
        let p = ((frac * PHASES as f64) as usize).min(PHASES - 1);
        let row = &table[p * 32..p * 32 + 32];
        let mut acc = 0f32;
        let start = i0 - TAPS + 1;
        if start >= 0 && start + 2 * TAPS <= n {
            let s = &x[start as usize..(start + 2 * TAPS) as usize];
            for k in 0..32 { acc += s[k] * row[k]; }
        } else {
            for k in 0..2 * TAPS {
                let j = start + k;
                if j >= 0 && j < n { acc += x[j as usize] * row[k as usize]; }
            }
        }
        *o = acc;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resample_keeps_pitch_and_length() {
        let from = 44100.0;
        let x: Vec<f32> = (0..44100).map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / from).sin() as f32 * 0.5).collect();
        let y = resample(&x, from, 48000.0);
        assert_eq!(y.len(), 48000);
        // zero crossings: 1000 Hz at 48 kHz
        let zc = y[1000..47000].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((zc as i64 - 958).abs() <= 2, "{zc}");
        let peak = y[1000..47000].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
    }
}
