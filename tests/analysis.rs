//! Analysis accuracy on the synthetic tracks the web version is tested with
//! (cratebox/tests/music, made by cratebox/tests/make_tracks.py): BPM, beat grid, key, phrases.
use cratebox::analysis::{analyze, mono_for, Opts, BPM_RANGES};
use cratebox::keys::camelot;
use std::path::PathBuf;

fn music() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/music");
    if p.exists() { p } else { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cratebox/tests/music") }
}

#[test]
fn synthetic_tracks() {
    let cases = [
        ("Test Artist - House Groove 126.wav", 126.0, 0.37, "8A"),
        ("Bass Lab - Halftime Wobble 140.wav", 140.0, 0.05, "4A"),
        ("Jungle Unit - Rollers 174.wav", 174.0, 0.21, "7A"),
        ("Boom Bap Crew - Dusty 92.wav", 92.0, 0.5, "5B"),
    ];
    let range = (BPM_RANGES[3].1, BPM_RANGES[3].2);
    let mut exact_keys = 0;
    for (name, bpm, off, key) in cases {
        let d = cratebox::decode::decode(&music().join(name)).expect(name);
        let (mono, fs) = mono_for(&[&d.l, &d.r], d.sr, 11025.0);
        let (w, fsw) = mono_for(&[&d.l, &d.r], d.sr, 22050.0);
        let a = analyze(&mono, fs, Some((&w, fsw)), range, Opts::default());
        let spb = 60.0 / bpm;
        let e = (a.first_beat - off).abs() % spb;
        let phase_err = e.min(spb - e);
        let k = camelot(a.key).map(|c| c.label()).unwrap_or_default();
        println!("{name}: bpm {:.3} first {:.3} (phase err {:.1} ms) key {k} phrases {:?}", a.bpm, a.first_beat, phase_err * 1000.0,
            a.phrases.iter().map(|p| (p.label.clone(), ((p.start - a.first_beat) / (240.0 / a.bpm)).round(), p.bars)).collect::<Vec<_>>());
        assert!((a.bpm - bpm).abs() < 0.05, "{name}: BPM {}", a.bpm);
        assert!(phase_err < 0.02, "{name}: grid off by {:.0} ms", phase_err * 1000.0);
        // i-VI-III-VII uses exactly the relative major's notes: the Camelot number must be right
        assert_eq!(k[..k.len() - 1], key[..key.len() - 1], "{name}: key {k}");
        if k == key { exact_keys += 1; }
        let w = a.wave.unwrap();
        assert!((w.peak.len() as f64 - d.duration() * 300.0).abs() < 2.0);
        if name.contains("126") {
            let ph: Vec<(String, i64)> = a.phrases.iter().map(|p| (p.label.clone(), ((p.start - a.first_beat) / (240.0 / a.bpm)).round() as i64)).collect();
            assert_eq!(ph, vec![("Intro".into(), 0), ("Drop".into(), 8), ("Breakdown".into(), 20), ("Drop".into(), 32)], "phrases");
        }
    }
    assert!(exact_keys >= 3, "minor/major right on {exact_keys}/4");
}
