//! End-to-end, four decks: deck 3 plays on its own channel, syncs to deck 1, the FLX4's layer button
//! moves the left side onto it, BEAT FX and PAD FX run on it, REC writes a WAV and HEAT's simulator
//! drives PUFF. A file of its own so CRATEBOX_DATA doesn't race tests/app.rs.

use cratebox::app::App;
use cratebox::fx::FxKind;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }
fn music() -> PathBuf {
    let p = root().join("tests/music");
    if p.exists() { p } else { root().join("../cratebox/tests/music") }
}
fn wait(app: &mut App, secs: f64, what: &str, mut f: impl FnMut(&App) -> bool) {
    let t0 = Instant::now();
    while !f(app) {
        assert!(t0.elapsed().as_secs_f64() < secs, "timed out waiting for {what}");
        app.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn run(app: &mut App, secs: f64) { let t0 = Instant::now(); while t0.elapsed().as_secs_f64() < secs { app.poll(); std::thread::sleep(Duration::from_millis(5)); } }
fn midi(app: &mut App, b: &[u8]) { if let Some(a) = app.midi.input(b) { app.midi_action(a); } app.poll(); }

#[test]
fn four_decks() {
    let data = std::env::temp_dir().join(format!("cratebox-four-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let rec = data.join("rec");
    std::fs::write(data.join("settings.json"), format!(r#"{{"folders":[{:?}],"four":true,"rec_dir":{:?}}}"#, music(), rec)).unwrap();
    std::env::set_var("CRATEBOX_DATA", &data);

    let mut app = App::new();
    assert!(app.four() && app.deck_active(3));
    wait(&mut app, 120.0, "analysis", |a| a.tracks.len() == 4 && a.tracks.iter().all(|t| t.analysis_current()));
    let id = |a: &App, n: &str| a.tracks.iter().find(|t| t.title.ends_with(n)).unwrap().id.clone();
    let (a126, a140) = (id(&app, "126"), id(&app, "140"));
    app.load_id(0, &a126);
    app.load_id(2, &a140);
    wait(&mut app, 30.0, "decks 1 and 3", |a| a.decks[0].loaded() && a.decks[2].loaded());

    // deck 3 plays on channel 3 and syncs to deck 1
    app.set_playing(0, true);
    run(&mut app, 0.3);
    app.set_sync(2, true);
    app.set_playing(2, true);
    run(&mut app, 1.5);
    assert!(app.status_snap.decks[2].playing && app.status_snap.decks[2].pos > 0.5, "deck 3 plays: {:?}", app.status_snap.decks[2]);
    assert!(app.status_snap.meters[2] > 0.0, "channel 3 meter");
    assert_eq!(app.tempo_master(), Some(0));
    assert!((app.decks[2].eff_bpm() - 126.0).abs() < 0.2, "deck 3 at {}", app.decks[2].eff_bpm());

    // the FLX4's SMART CFX as the left layer button: the left side now plays deck 3
    app.use_profile("ddj-flx4");
    app.midi.setup_layer_buttons();
    midi(&mut app, &[0x96, 0x01, 0x7f]);
    midi(&mut app, &[0x96, 0x02, 0x7f]);
    midi(&mut app, &[0x96, 0x01, 0x7f]);
    assert_eq!(app.midi.side[0], 2);
    midi(&mut app, &[0x90, 0x0b, 0x7f]); // left PLAY = deck 3 pause
    run(&mut app, 0.2);
    assert!(!app.decks[2].playing && app.decks[0].playing);

    // BEAT FX echo on channel 1, then a PAD FX roll on deck 1 while held
    app.fx_channel(0);
    app.fx_pick(FxKind::Echo);
    app.fx_on(true);
    run(&mut app, 0.5);
    app.fx_on(false);
    app.decks[0].pad_mode = cratebox::deck::PadMode::PadFx;
    app.pad_down(0, 3, false);
    assert!(app.fx.pad[0].is_some());
    run(&mut app, 0.3);
    app.pad_up(0, 3);
    assert!(app.fx.pad[0].is_none());

    // REC: the master to a WAV
    app.toggle_rec();
    assert!(app.rec.is_some());
    run(&mut app, 1.0);
    app.toggle_rec();
    std::thread::sleep(Duration::from_millis(700));
    let wav = std::fs::read_dir(&rec).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == "wav")).expect("a recording").path();
    assert!(std::fs::metadata(&wav).unwrap().len() > 44 + 48000, "{} is too short", wav.display());

    // HEAT: the simulator puts PUFF on offer; taking it away goes back to ECHO
    app.heat.use_simulator();
    wait(&mut app, 5.0, "simulator", |a| a.fx.puff_ready);
    app.fx_pick(FxKind::Puff);
    app.heat.disconnect();
    wait(&mut app, 5.0, "puffco gone", |a| !a.fx.puff_ready);
    assert_eq!(app.fx.fx, FxKind::Echo);

    // two decks again: deck 3 stops and hides
    app.set_four(false);
    assert!(!app.deck_active(2) && !app.decks[2].playing);
    app.flush_now();
}
