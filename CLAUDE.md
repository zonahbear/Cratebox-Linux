# CLAUDE.md: Cratebox-Linux

Read this first; it exists so you don't have to re-parse the repo. Update it when you change structure,
conventions, or finish/start a piece of open work. Keep it under ~150 lines.

## What this repo is

Native Linux port of the Cratebox web app in Rust (eframe/egui on glow, cpal for audio, midir for MIDI,
symphonia for decoding). Two decks, DDJ-FLX4 first, other Pioneer/AlphaTheta controllers via Mixxx-derived maps.
Same behaviour and analysis as the web version, no browser or server. v0.1.0: passes its tests, not yet tried
on real sound hardware or a controller. Built for x86-64-v3 CPUs.

## Map

```
src/main.rs        entry; CPU feature check, --version, --screenshot FILE [FRAMES] (used by tests)
src/lib.rs         module list (the binary and tests both use the lib)
src/app.rs         App: decks, mixer, library, playlists, controller, background jobs; `poll` drains job results each frame
src/ui.rs          the window: header, waveforms, decks + mixer, library (largest file)
src/widgets.rs     custom-drawn knobs, faders, jogs, meters, waveform painters
src/dialog.rs      egui file/folder picker (no GTK/portal)
src/engine.rs      audio engine: 2 players → mixer; commands in by channel, try-locked snapshot out
src/player.rs      deck player on the audio thread, WSOLA keylock; port of the web AudioWorklet (128-sample blocks)
src/mixer.rs       trim, 3-band isolator EQ, CFX filter, cue, faders, limiter, routing
src/deck.rs        deck logic on the UI thread: transport, cue, sync, quantize, loops, hot cues, jog, grid editor
src/analysis.rs    BPM / grid / key / phrases / waveform bands; line-by-line port of the web analyser
src/fft.rs keys.rs FFT, Camelot keys
src/decode.rs      symphonia → planar f32 + tags
src/library.rs     folder scan, tags, analysis cache, search/sort
src/playlists.rs   playlists + folders, rekordbox USB (read-only), rekordbox XML import/export
src/rekordbox.rs   export.pdb + ANLZ readers, XML (crate-digger layouts)
src/midi.rs        controller profiles, table-driven MIDI engine, LEDs, MIDI learn
src/maps.rs        GENERATED from the web app's MIXXX_MAPS; don't hand-edit (regenerate instead)
src/midi_io.rs audio_io.rs   midir / cpal device handling (rescans, "no output" timer mode)
src/store.rs       JSON persistence in ~/.local/share/cratebox ($XDG_DATA_HOME, or $CRATEBOX_DATA)
src/ring.rs        lock-free ring buffer
src/uitest.rs      scripted UI driver: CRATEBOX_UITEST=script.json, output to CRATEBOX_UITEST_OUT
tests/analysis.rs  analysis accuracy on synthetic tracks
tests/app.rs       whole app end-to-end with FLX4 MIDI messages, persistence
tests/ui.rs        real window under Xvfb (skipped without xvfb-run)
tests/make_tracks.py   synthetic test tracks → tests/music (same script as the web repo)
tests/fixtures/    rekordbox USB export (pyrekordbox test data, MIT)
assets/fonts/      Inter + DejaVu (licences alongside); assets/icon-512.png
packaging/         install.sh, cratebox.desktop, cratebox.png (go into the release tarball)
build-release.sh   distributable tarball via cargo-zigbuild, glibc 2.31 baseline → dist/
env.sh             build env for one past sandbox (points at a local ALSA shim); not needed with libasound2-dev
```

## Build and test

- Needs ALSA dev files: `apt install build-essential pkg-config libasound2-dev` (plus `xvfb` for tests/ui.rs).
  In a cloud session, install those first; don't rely on `env.sh` (its shim path won't exist).
- `cargo build --release` (x86-64-v3 via `.cargo/config.toml`, target `x86_64-unknown-linux-gnu`, so the binary
  is at `target/x86_64-unknown-linux-gnu/release/cratebox`). Older CPU: `--config 'build.rustflags=[]'`.
- `python3 tests/make_tracks.py tests/music` (needs numpy) then `cargo test --release`.
  Tests fall back to `../cratebox/tests/music` if `tests/music` is missing.
- With no sound card the engine runs on a timer, so timing tests still work in containers.
- No CI in this repo. Releases are built by hand with `./build-release.sh`.

## Conventions

- `main` only; session branches are `claude/...`. Version in `Cargo.toml`.
- Module docs: every file opens with `//!` lines saying what it is and which thread it runs on. Keep that.
- Audio thread code (player, mixer, engine callback) never blocks: try-lock, no allocation.
- Ports stay close to the web code (same names, same maths) so results match; `analysis.rs` and `player.rs`
  are meant to be line-by-line ports.
- Commit messages: plain sentence of what changed for the user.

## Cross-repo

- Behaviour reference is **zonahbear/Cratebox** `cratebox.html` (see that repo's CLAUDE.md for its section map).
  When the web app's analysis, deck logic or controller maps change, this port drifts; nothing checks it
  automatically.
- `src/maps.rs` comes from the web app's `MIXXX_MAPS` table (itself from `tools/mixxx2cratebox.py` there).
- **cratebox-iOS-suite** is a separate Swift port; no shared code.

## Gotchas

- Two decks only (the web and iOS apps have 4-deck mode and deck layers; not ported).
- rekordbox is read-only here: never writes to a stick (the web app's playlist writer isn't ported).
- Binary refuses to run on CPUs without AVX2/FMA/BMI2 and says how to rebuild.
- `build-release.sh` links with `--allow-shlib-undefined` on purpose (user's own libasound at run time).

## Open work (update this when it changes)

- Untested on real audio hardware and a real FLX4.
- Behind the web app: 4 decks / deck layers, Beat FX & Pad FX, recording, stick playlist editing, other
  1.1.x changes. Check the web CHANGELOG against this port before claiming parity.
