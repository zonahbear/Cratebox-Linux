//! End-to-end: the whole app (everything but the drawing), driven the way the FLX4 and the mouse
//! drive it. The same checks as the web version's suites: analysis, MIDI, playback, sync, cues,
//! loops, grid edits, playlists, persistence, rekordbox XML + USB, MIDI learn.
//! With no sound card (CI, containers) the engine runs on its timer, so timing checks still hold.

use cratebox::app::App;
use cratebox::keys::camelot;
use cratebox::library::SortKey;
use cratebox::playlists::View;
use std::path::{Path, PathBuf};
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
fn whole_app() {
    let data = std::env::temp_dir().join(format!("cratebox-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    // a music folder of our own (a copy: XML matching goes by file name + size)
    let mdir = data.join("music");
    std::fs::create_dir_all(&mdir).unwrap();
    for e in std::fs::read_dir(music()).unwrap().flatten() { std::fs::copy(e.path(), mdir.join(e.file_name())).unwrap(); }
    std::fs::write(data.join("settings.json"), format!(r#"{{"folders":[{:?}]}}"#, mdir)).unwrap();
    std::env::set_var("CRATEBOX_DATA", &data);

    let mut app = App::new();
    wait(&mut app, 120.0, "analysis", |a| a.tracks.len() == 4 && a.tracks.iter().all(|t| t.analysis_current()));
    let want = [("126", 126.0, "8"), ("140", 140.0, "4"), ("174", 174.0, "7"), ("92", 92.0, "5")];
    for (n, bpm, key) in want {
        let t = app.tracks.iter().find(|t| t.title.ends_with(n)).unwrap();
        assert!((t.bpm.unwrap() - bpm).abs() < 0.05, "{} bpm {:?}", t.title, t.bpm);
        let k = camelot(t.key.unwrap()).unwrap();
        assert_eq!(k.num.to_string(), key, "{} key", t.title);
    }
    assert!(app.tracks.iter().any(|t| t.artist == "Bass Lab" && t.title == "Halftime Wobble 140"), "tags from file names");

    // ---- FLX4: browse + LOAD
    app.use_profile("ddj-flx4");
    app.list.sort = Some(SortKey::Bpm); app.list.dir = 1; app.apply_view(false);
    app.list.sel = 0;
    midi(&mut app, &[0xb6, 0x40, 0x01]); // browse one click clockwise -> row 2 (126)
    assert_eq!(app.list.sel, 1);
    midi(&mut app, &[0x96, 0x46, 0x7f]); // LOAD deck 1
    wait(&mut app, 30.0, "deck 1 load", |a| a.decks[0].loaded());
    midi(&mut app, &[0xb6, 0x40, 0x01]);
    midi(&mut app, &[0x96, 0x47, 0x7f]); // LOAD deck 2
    wait(&mut app, 30.0, "deck 2 load", |a| a.decks[1].loaded());
    assert!(app.decks[0].track.as_ref().unwrap().title.contains("126") && app.decks[1].track.as_ref().unwrap().title.contains("140"));
    let fb = app.decks[0].ana.as_ref().unwrap().first_beat;
    assert!((app.decks[0].cue - fb).abs() < 0.01, "cue point defaults to the first beat");

    // ---- mixer: 14-bit fader (MSB + LSB), crossfader, EQ kill
    midi(&mut app, &[0xb0, 0x13, 0x7f]); midi(&mut app, &[0xb0, 0x33, 0x7f]);
    assert_eq!(app.mixer.fader[0], 1.0);
    midi(&mut app, &[0xb6, 0x1f, 0x00]); midi(&mut app, &[0xb6, 0x3f, 0x00]);
    assert_eq!(app.mixer.xfader, 0.0);
    midi(&mut app, &[0xb0, 0x0f, 0x00]); midi(&mut app, &[0xb0, 0x2f, 0x00]);
    assert_eq!(app.mixer.low[0], 0.0);
    midi(&mut app, &[0xb0, 0x0f, 0x40]); midi(&mut app, &[0xb6, 0x1f, 0x40]);

    // ---- PLAY moves at the right speed, and the PLAY LED answers
    let c0 = app.decks[0].now();
    midi(&mut app, &[0x90, 0x0b, 0x7f]);
    run(&mut app, 1.5);
    let moved = app.decks[0].now() - c0;
    assert!(app.decks[0].playing && (1.3..1.7).contains(&moved), "PLAY: moved {moved:.2}s in 1.5 s");
    // ---- tempo fader: top = -6%
    midi(&mut app, &[0xb0, 0x00, 0x00]); midi(&mut app, &[0xb0, 0x20, 0x00]);
    assert!((app.decks[0].rate() - 0.94).abs() < 0.001, "tempo top = -6%: {}", app.decks[0].rate());
    midi(&mut app, &[0xb0, 0x00, 0x40]); midi(&mut app, &[0xb0, 0x20, 0x00]);
    assert!((app.decks[0].rate() - 1.0).abs() < 0.001);

    // ---- SYNC: deck 2 (140) follows deck 1 (126), beats line up once it plays
    midi(&mut app, &[0x91, 0x58, 0x7f]);
    assert!(app.decks[1].sync && (app.decks[1].eff_bpm() - 126.0).abs() < 0.01, "sync: {}", app.decks[1].eff_bpm());
    midi(&mut app, &[0x91, 0x0b, 0x7f]);
    run(&mut app, 1.0);
    let (b0, b1) = (app.decks[0].beat_at(app.decks[0].now()), app.decks[1].beat_at(app.decks[1].now()));
    let ph = ((b0 - b1).rem_euclid(1.0)).min(1.0 - (b0 - b1).rem_euclid(1.0));
    assert!(ph < 0.03, "synced decks in phase: {ph:.3} beat apart");
    // moving deck 1's tempo carries deck 2
    midi(&mut app, &[0xb0, 0x00, 0x7f]); midi(&mut app, &[0xb0, 0x20, 0x7f]);
    app.poll();
    assert!((app.decks[1].eff_bpm() - app.decks[0].eff_bpm()).abs() < 0.01);
    midi(&mut app, &[0xb0, 0x00, 0x40]); midi(&mut app, &[0xb0, 0x20, 0x00]);

    // ---- hot cue A on deck 1 (set, then jump back to it)
    midi(&mut app, &[0x97, 0x00, 0x7f]);
    let a = app.decks[0].hot[0].expect("hot cue A set");
    let spb = app.decks[0].spb();
    assert!(((a - app.decks[0].ana.as_ref().unwrap().first_beat) / spb).fract().abs() < 0.01 || ((a - fb) / spb).fract() > 0.99, "quantized to a beat");
    run(&mut app, 1.0);
    midi(&mut app, &[0x97, 0x00, 0x7f]);
    assert!((app.decks[0].now() - a).abs() < 0.1 || ((app.decks[0].now() - a) / spb).fract().abs() < 0.05, "hot cue jump");
    // ---- 4-beat loop from the pads (beat loop mode)
    midi(&mut app, &[0x90, 0x6d, 0x7f]);
    midi(&mut app, &[0x97, 0x64, 0x7f]);
    let (la, lb) = (app.decks[0].lp.a.unwrap(), app.decks[0].lp.b.unwrap());
    assert!(app.decks[0].lp.on && ((lb - la) / spb - 4.0).abs() < 0.01);
    run(&mut app, 2.5);
    let t = app.decks[0].now();
    assert!(t >= la - 0.05 && t <= lb + 0.05, "stays in the loop: {t:.2} in {la:.2}..{lb:.2}");
    midi(&mut app, &[0x90, 0x4d, 0x7f]); // RELOOP/EXIT
    assert!(!app.decks[0].lp.on);
    // ---- CUE while playing: back to the cue point and stop
    midi(&mut app, &[0x90, 0x0c, 0x7f]); midi(&mut app, &[0x80, 0x0c, 0x00]);
    assert!(!app.decks[0].playing && (app.decks[0].now() - app.decks[0].cue).abs() < 0.05);

    // ---- grid edit is saved with the track (and survives a restart, below)
    let id0 = app.decks[0].track.as_ref().unwrap().id.clone();
    app.decks[0].set_bpm(128.0, "test");
    app.poll();
    assert!(app.store.cache[&id0].grid_manual && app.store.cache[&id0].bpm == Some(128.0));

    // ---- master tempo: +8% with keylock keeps the key; off = vinyl (key shifts)
    app.set_sync(1, false);
    app.decks[1].set_pitch(1.0);
    app.set_mt(1, false);
    let k_vinyl = app.decks[1].key_now();
    app.set_mt(1, true);
    assert_ne!(k_vinyl, app.decks[1].key_now());
    app.decks[1].set_pitch(0.0);
    midi(&mut app, &[0x91, 0x0b, 0x7f]); // stop deck 2

    // ---- browse press: tree focus, turning walks it, press again goes back
    let pl = app.lists.create("Bangers", false, None);
    let sel: Vec<_> = app.list.view.iter().take(3).cloned().collect();
    app.lists.add(&pl, &sel, None);
    midi(&mut app, &[0x96, 0x41, 0x7f]);
    assert!(app.focus_tree);
    midi(&mut app, &[0xb6, 0x40, 0x01]); // -> PLAYLISTS head is skipped -> Bangers
    assert_eq!(app.view, View::Pl(pl.clone()));
    midi(&mut app, &[0x96, 0x41, 0x7f]);
    assert!(!app.focus_tree);
    assert_eq!(app.list.view.len(), 3);

    // ---- playlists: rearrange, remove, folder
    let first = app.list.view[0].id.clone();
    app.move_in_playlist(&[0], 3);
    assert_eq!(app.list.view[2].id, first);
    app.list.click(0, false, false);
    app.remove_from_playlist();
    assert_eq!(app.list.view.len(), 2);
    let f = app.lists.create("Saturday", true, None);
    assert!(app.lists.move_to(&pl, Some(f.clone())));

    // ---- rekordbox XML: import (grid, key, cues) and export
    let house = app.tracks.iter().find(|t| t.title.ends_with("126")).unwrap().clone();
    let xml = format!(r#"<?xml version="1.0" encoding="UTF-8"?><DJ_PLAYLISTS Version="1.0.0"><COLLECTION Entries="1">
<TRACK TrackID="1" Name="House" Artist="T" Size="{}" TotalTime="75" AverageBpm="126.00" Tonality="Am" Location="file://localhost/C:/Music/{}">
<TEMPO Inizio="0.367" Bpm="126.00" Metro="4/4" Battito="1"/><POSITION_MARK Name="" Type="0" Start="20.0" Num="2"/></TRACK></COLLECTION>
<PLAYLISTS><NODE Type="0" Name="ROOT" Count="1"><NODE Name="From rekordbox" Type="1" KeyType="0" Entries="1"><TRACK Key="1"/></NODE></NODE></PLAYLISTS></DJ_PLAYLISTS>"#,
        house.size, house.name.replace(' ', "%20"));
    let xp = data.join("in.xml");
    std::fs::write(&xp, xml).unwrap();
    // the 126 track's own cues exist (hot cue A set above), so rekordbox's don't replace them; clear them to see the import
    app.store.cues.remove(&house.id);
    app.import_xml(&xp);
    let h = app.track(&house.id).unwrap();
    // this track's grid was edited by hand above (128): your own edit wins over rekordbox's; the key comes in
    assert!(h.grid_manual && h.bpm == Some(128.0) && h.key_from.as_deref() == Some("rekordbox") && h.key == Some(21));
    let other = app.tracks.iter().find(|t| t.title.ends_with("140")).unwrap().clone();
    let xml2 = format!(r#"<DJ_PLAYLISTS Version="1.0.0"><COLLECTION Entries="1"><TRACK TrackID="7" Name="W" Size="{}" Tonality="Fm" Location="file://localhost/x/{}">
<TEMPO Inizio="0.5" Bpm="140.00" Metro="4/4" Battito="2"/></TRACK></COLLECTION><PLAYLISTS><NODE Type="0" Name="ROOT" Count="0"></NODE></PLAYLISTS></DJ_PLAYLISTS>"#, other.size, other.name.replace(' ', "%20"));
    std::fs::write(&xp, xml2).unwrap();
    app.import_xml(&xp);
    let o = app.track(&other.id).unwrap();
    // Battito 2: the beat at 0.5 s is the 2nd of the bar, so bar 1 is one beat earlier (and folded into the first bar)
    let fb = o.first_beat.unwrap();
    assert!(o.grid_from.as_deref() == Some("rekordbox") && o.bpm == Some(140.0) && ((fb - (0.5 - 60.0 / 140.0)).abs() < 1e-9), "rekordbox grid: {:?} {fb}", o.bpm);
    assert_eq!(app.store.cues[&house.id].hot[2], Some(20.0));
    assert!(app.lists.lists.iter().any(|p| p.name == "From rekordbox" && p.items == vec![house.id.clone()]));
    let xo = data.join("out.xml");
    app.export_xml(&xo);
    let out = std::fs::read_to_string(&xo).unwrap();
    assert!(out.contains(r#"<NODE Name="Bangers""#) && out.contains("<TEMPO") && out.contains(&cratebox::rekordbox::location_url(&house.path.to_string_lossy())));

    // ---- a rekordbox USB stick: the fixture export, with a playable "Demo Track 1"
    let usb = data.join("USBSTICK");
    copy_dir(&root().join("tests/fixtures/rekordbox_usb"), &usb);
    let audio_dir = usb.join("Contents/Loopmasters/UnknownAlbum");
    std::fs::create_dir_all(&audio_dir).unwrap();
    std::fs::copy(music().join("Test Artist - House Groove 126.wav"), audio_dir.join("Demo Track 1.mp3")).unwrap();
    app.mount(&usb, true);
    wait(&mut app, 10.0, "USB mount", |a| !a.lists.devices.is_empty());
    let dev = app.lists.devices[0].id.clone();
    app.set_view(View::Dev(dev.clone()));
    assert_eq!(app.list.view.len(), 6);
    let sub = app.lists.devices[0].tree.iter().find(|n| n.name == "Sub Playlist").unwrap().id;
    app.set_view(View::DevPl(dev.clone(), sub));
    assert_eq!(app.list.view.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Demo Track 1", "Demo Track 2"]);
    app.list.sel = 0;
    app.load_selected(1);
    wait(&mut app, 30.0, "USB track load", |a| a.decks[1].loaded() || (!a.decks[1].loading && a.decks[1].track.is_none()));
    let d = &app.decks[1];
    let ana = d.ana.as_ref().expect("USB track loaded");
    assert_eq!((ana.bpm, ana.first_beat), (128.0, 0.025), "rekordbox's grid");
    assert_eq!(ana.phrases.len(), 19, "rekordbox's phrases");
    assert_eq!(camelot(ana.key).unwrap().label(), "4A");
    assert!(app.track(&app.list.view[0].id).is_some());
    let notes = app.tracks.iter().filter(|t| t.device.is_none()).count();
    assert_eq!(notes, 4, "the collection doesn't mix device tracks in");

    // ---- MIDI learn on an unknown controller
    app.use_profile("learn");
    app.start_learn();
    app.learn_pick("play", 1);
    midi(&mut app, &[0x9f, 0x10, 0x7f]);
    app.stop_learn();
    assert_eq!(app.store.settings.learned["learn"].len(), 1);

    // ---- everything comes back after a restart
    let n_pl = app.lists.lists.len();
    app.flush_now();
    drop(app);
    let mut app = App::new();
    wait(&mut app, 30.0, "rescan", |a| a.tracks.iter().filter(|t| t.device.is_none()).count() == 4 && !a.lists.devices.is_empty());
    app.poll();
    assert_eq!(app.lists.lists.len(), n_pl, "playlists kept");
    assert!(app.track(&id0).unwrap().grid_manual, "grid edit kept");
    assert!(app.tracks.iter().filter(|t| t.device.is_none()).all(|t| t.analysis_current()), "analysis cache used: nothing re-analysed");
    assert_eq!(app.store.cues[&house.id].hot[2], Some(20.0), "cues kept");
    assert_eq!(app.store.settings.learned["learn"].len(), 1, "learned mapping kept");
    let _ = std::fs::remove_dir_all(&data);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { copy_dir(&p, &to.join(e.file_name())); } else { std::fs::copy(&p, to.join(e.file_name())).unwrap(); }
    }
}
