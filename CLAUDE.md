# CLAUDE.md: Cratebox-Linux

Read this first; it exists so you don't have to re-parse the repo. Update it when you change structure,
conventions, or finish/start a piece of open work. Keep it under ~150 lines.

## What this repo is

Native Linux port of the Cratebox web app (and the iOS performance mode) in Rust (eframe/egui on glow, cpal for audio, midir for MIDI,
symphonia for decoding). Two or four decks (deck layers 1⇄3 / 2⇄4), DDJ-FLX4 first, other Pioneer/AlphaTheta controllers via Mixxx-derived maps.
Same behaviour and analysis as the web version, no browser or server. v0.1.0: passes its tests, not yet tried
on real sound hardware or a controller. Built for x86-64-v3 CPUs.

## Map

```
src/main.rs        entry; CPU feature check, --version, --screenshot FILE [FRAMES] (used by tests), --kiosk (full screen)
src/lib.rs         module list (the binary and tests both use the lib)
src/app.rs         App: decks, mixer, library, playlists, controller, background jobs; `poll` drains job results each frame
src/ui.rs          the window, XDJ-style, sized for 1152×720 pt (MacBook10,1 at 2x): header (REC, WAVE, 4 DECKS,
                   HEAT ▾, ⚙), stacked waveform lanes with info strips, decks either side of the mixer (3 1 2 4
                   + BEAT FX), library; WAVE = only the lanes; BROWSE = lanes + big library; HEAT panel
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
src/fx.rs          Beat FX / Pad FX DSP (FxUnit: one per channel insert + master), port of the iOS FxUnit
src/beatfx.rs      BEAT FX unit + PAD FX state on the UI thread; sends beat clocks every 100 ms
src/recorder.rs    REC: master → 16-bit WAV (ring from the audio thread, writer thread)
src/heat.rs        HEAT (TrueCurve mini): curve maths, driver, learner, PuffDrop, simulator, Lorax bytes,
                   BlueZ GATT via zbus (pure Rust, no libdbus), worker thread; port of iOS personal/heat
src/uitest.rs      scripted UI driver: CRATEBOX_UITEST=script.json, output to CRATEBOX_UITEST_OUT
tests/analysis.rs  analysis accuracy on synthetic tracks
tests/app.rs       whole app end-to-end with FLX4 MIDI messages, persistence
tests/ui.rs        real window under Xvfb (skipped without xvfb-run)
docs/keyboard.md   keyboard + trackpad shortcuts (same list as ui.rs SHORTCUTS, the F12 window)
tests/four.rs      4 decks end-to-end: channel 3, sync, layer buttons, FX, REC, HEAT simulator → PUFF
tests/make_tracks.py   synthetic test tracks → tests/music (same script as the web repo)
tests/fixtures/    rekordbox USB export (pyrekordbox test data, MIT)
assets/fonts/      Inter + DejaVu (licences alongside); assets/icon-512.png
packaging/         install.sh, cratebox.desktop, cratebox.png (go into the release tarball)
build-release.sh   distributable tarball via cargo-zigbuild, glibc 2.31 baseline → dist/
env.sh             build env for one past sandbox (points at a local ALSA shim); not needed with libasound2-dev
distro/            Cratebox OS ISO: build-iso.sh remasters Lubuntu 26.04.1 (layered squashfs: chroot-setup.sh runs
                   in the minimal.standard layer, live-setup.sh in the live layer), overlay/ = files for the
                   installed system, live/ = live-session script. distro/README.md = guide + verified/proposed table.
                   plymouth/make-theme.py draws the boot splash PNGs (committed). Calamares "Minimal" always unpacks
                   the full layer, then cratebox-minimal-install autoremoves Lubuntu's extras (list made in chroot-setup)
.github/workflows/cratebox-os.yml  manual-only ubuntu job → release "kiosk-latest" (ISO in <2 GiB parts)
```

## Build and test

- Needs ALSA dev files: `apt install build-essential pkg-config libasound2-dev` (plus `xvfb` for tests/ui.rs).
- Realtime audio: `promote_to_realtime` in src/audio_io.rs (direct SCHED_FIFO, needs rtprio). Don't go back to cpal's
  `audio_thread_priority`: it sets RLIMIT_RTTIME, and SIGXCPU killed Cratebox when the callback overran.
  In a cloud session, install those first; don't rely on `env.sh` (its shim path won't exist).
- `cargo build --release` (x86-64-v3 via `.cargo/config.toml`, target `x86_64-unknown-linux-gnu`, so the binary
  is at `target/x86_64-unknown-linux-gnu/release/cratebox`). Older CPU: `--config 'build.rustflags=[]'`.
- `python3 tests/make_tracks.py tests/music` (needs numpy) then `cargo test --release`.
  Tests fall back to `../cratebox/tests/music` if `tests/music` is missing.
- With no sound card the engine runs on a timer, so timing tests still work in containers.
- No CI on pushes. Releases are built by hand with `./build-release.sh`; the Cratebox OS ISO by the manual
  cratebox-os workflow (Actions minutes are scarce: keep workflows `workflow_dispatch`).

## Conventions

- `main` only; session branches are `claude/...`. Version in `Cargo.toml`.
- Module docs: every file opens with `//!` lines saying what it is and which thread it runs on. Keep that.
- Audio thread code (player, mixer, engine callback) never blocks: try-lock, no allocation.
- Ports stay close to the web code (same names, same maths) so results match; `analysis.rs` and `player.rs`
  are meant to be line-by-line ports.
- Commit messages: plain sentence of what changed for the user.
- Cratebox OS is audio first, for Jonah's MacBook10,1: keep the ISO's kernel (7.0.0-30, verified with the
  speaker driver) and hold it; s2idle only; never TLP or HDA power_save tweaks; never remove
  pipewire/wireplumber/rtkit. StartupMute=%00 is per-machine NVRAM (first-boot step in distro/README.md).

## Cross-repo

- Behaviour reference is **zonahbear/Cratebox** `cratebox.html` (see that repo's CLAUDE.md for its section map).
  When the web app's analysis, deck logic or controller maps change, this port drifts; nothing checks it
  automatically.
- `src/maps.rs` comes from the web app's `MIXXX_MAPS` table (itself from `tools/mixxx2cratebox.py` there).
- **cratebox-iOS-suite** is a separate Swift port; no shared code.

## Gotchas

- Layer buttons are learned once (Settings → Controller → Set up layer buttons…): the FLX4's SMART CFX/FADER
  notes aren't in the map. Their LEDs stay off.
- Decks are XDJ-style screens (platter, time, key, BPM, beat/phrase, hot cues) with buttons under them, or
  screens only (Settings → Decks; auto = screens only while a MIDI controller is connected; `deck_view`).
- HEAT registers its own BlueZ agent (NoInputNoOutput, /org/cratebox/agent) since the kiosk has no desktop one;
  HEAT's messages go to heat.log in the data dir.
- HEAT safety rules (slot 4 backed up/restored, 600°F cutoff, 350-590°F clamp) must match zonahbear/truecurve.
- CRATEBOX_DEMO=1 (+ CRATEBOX_DEMO_HEAT=1) drive the screenshots; 4-deck demo loads decks 3/4 too.
- rekordbox is read-only here: never writes to a stick (the web app's playlist writer isn't ported).
- Binary refuses to run on CPUs without AVX2/FMA/BMI2 and says how to rebuild.
- `build-release.sh` links with `--allow-shlib-undefined` on purpose (user's own libasound at run time).

## Open work (update this when it changes)

- Untested on real audio hardware and a real FLX4; HEAT's BlueZ link untested on a real Puffco (simulator only).
- Cratebox OS: builds on Actions (cratebox-os → release kiosk-latest). On the MacBook (Jonah): live speakers work;
  sleep/wake reconnect, a full install (normal and minimal), headphones, Claude sign-in and HEAT still to check.
  The "proposed" settings in distro/README.md are unmeasured.
- Behind the web app: stick playlist editing, other
  1.1.x changes. Check the web CHANGELOG against this port before claiming parity.
