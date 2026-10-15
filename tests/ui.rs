//! The real window, driven with real mouse and keyboard events (see src/uitest.rs), under Xvfb.
//! Skipped when xvfb-run isn't installed.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn music() -> PathBuf {
    let r = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let p = r.join("tests/music");
    if p.exists() { p } else { r.join("../cratebox/tests/music") }
}

#[test]
fn click_through_the_window() {
    if Command::new("xvfb-run").arg("--help").output().is_err() { eprintln!("xvfb-run not installed: skipped"); return; }
    let data = std::env::temp_dir().join(format!("cratebox-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), format!(r#"{{"folders":[{:?}]}}"#, music())).unwrap();
    let script = r#"[
      {"do":"wait","until":"tracks_ready","timeout":90},
      {"do":"frames","n":5},
      {"do":"dump","name":"start"},
      {"do":"click","target":"row3","double":true},
      {"do":"wait","until":"deck0_loaded"},
      {"do":"drag","from":"row0","to":"deck1.area"},
      {"do":"wait","until":"deck1_loaded"},
      {"do":"frames","n":5},
      {"do":"dump","name":"loaded"},
      {"do":"click","target":"deck0.play"},
      {"do":"frames","n":60},
      {"do":"dump","name":"playing"},
      {"do":"click","target":"deck0.play"},
      {"do":"frames","n":5},
      {"do":"dump","name":"paused"},
      {"do":"drag","from":"lane0","dx":-200},
      {"do":"frames","n":3},
      {"do":"dump","name":"scrubbed"},
      {"do":"click","target":"deck1.overview","fx":0.5},
      {"do":"frames","n":3},
      {"do":"dump","name":"overview"},
      {"do":"click","target":"deck0.pad0"},
      {"do":"frames","n":2},
      {"do":"drag","from":"deck0.fader","dy":40},
      {"do":"drag","from":"deck0.low","dy":60},
      {"do":"click","target":"deck1.sync"},
      {"do":"frames","n":3},
      {"do":"dump","name":"controls"},
      {"do":"click","target":"tree.newpl"},
      {"do":"frames","n":3},
      {"do":"key","key":"A","ctrl":true},
      {"do":"type","text":"UI list"},
      {"do":"key","key":"Enter"},
      {"do":"frames","n":3},
      {"do":"dump","name":"newpl"},
      {"do":"click","target":"tree:Collection"},
      {"do":"frames","n":3},
      {"do":"drag","from":"row1","to":"tree:UI list"},
      {"do":"frames","n":3},
      {"do":"dump","name":"added"},
      {"do":"click","target":"row0"},
      {"do":"key","key":"ArrowDown"},
      {"do":"key","key":"ArrowDown"},
      {"do":"frames","n":2},
      {"do":"dump","name":"keys"},
      {"do":"click","target":"settings"},
      {"do":"frames","n":3},
      {"do":"dump","name":"settings"}
    ]"#;
    std::fs::write(data.join("script.json"), script).unwrap();
    let out = data.join("out.json");
    let status = Command::new("xvfb-run").args(["-a", "-s", "-screen 0 1440x900x24"]).arg(env!("CARGO_BIN_EXE_cratebox"))
        .env("CRATEBOX_DATA", &data).env("CRATEBOX_UITEST", data.join("script.json")).env("CRATEBOX_UITEST_OUT", &out)
        .stderr(std::process::Stdio::null()).status().expect("run cratebox");
    assert!(status.success(), "cratebox exited with {status}");
    let v: Value = serde_json::from_slice(&std::fs::read(&out).expect("no test output")).unwrap();
    let log = v["log"].as_array().unwrap();
    assert!(log.is_empty(), "script problems: {log:?}");
    let s = &v["snaps"];
    let d = |snap: &str, i: usize, k: &str| s[snap]["decks"][i][k].clone();
    assert_eq!(s["start"]["list"]["titles"].as_array().unwrap().len(), 4);
    // double-click loads deck 1; dragging a row onto deck 2 loads it there
    assert_eq!(d("loaded", 0, "title"), "House Groove 126");
    assert_eq!(d("loaded", 1, "title"), "Halftime Wobble 140");
    // PLAY runs for ~1 s of frames, then pause
    assert_eq!(d("playing", 0, "playing"), true);
    assert!(d("playing", 0, "now").as_f64().unwrap() > d("loaded", 0, "now").as_f64().unwrap() + 0.3);
    assert_eq!(d("paused", 0, "playing"), false);
    // dragging the paused waveform left moves later in the track (200 px at 8 s per lane width)
    let moved = d("scrubbed", 0, "now").as_f64().unwrap() - d("paused", 0, "now").as_f64().unwrap();
    assert!((0.9..1.4).contains(&moved), "scrub moved {moved}");
    // clicking the middle of the overview jumps to the middle
    let (now, dur) = (d("overview", 1, "now").as_f64().unwrap(), d("overview", 1, "duration").as_f64().unwrap());
    assert!((now / dur - 0.5).abs() < 0.02, "overview click: {}", now / dur);
    // pad A sets a hot cue; fader and EQ follow the mouse; SYNC on
    assert!(d("controls", 0, "hot")[0].is_number());
    assert!(s["controls"]["mixer"]["fader"][0].as_f64().unwrap() < 0.75);
    assert!(s["controls"]["mixer"]["low"][0].as_f64().unwrap() < 0.4);
    assert_eq!(d("controls", 1, "sync"), true);
    assert!((d("controls", 1, "rate").as_f64().unwrap() * 140.0 - 126.0).abs() < 0.05);
    // new playlist, named by typing, opened; a row dragged onto it in the tree
    let pls = s["newpl"]["playlists"].as_array().unwrap();
    assert!(pls.iter().any(|p| p["name"] == "UI list"), "{pls:?}");
    assert_eq!(s["newpl"]["view"], "UI list");
    let pl = s["added"]["playlists"].as_array().unwrap().iter().find(|p| p["name"] == "UI list").unwrap().clone();
    assert_eq!(pl["items"], 1);
    // arrow keys move the selection
    assert_eq!(s["keys"]["list"]["sel"], 2);
    assert_eq!(s["settings"]["settings_open"], true);
    let _ = std::fs::remove_dir_all(&data);
}
