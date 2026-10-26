# Cratebox for Linux

Version 0.1.0 (early: runs and passes its tests, not yet tried on real sound hardware or a controller).

Two-deck DJ software for the Pioneer DDJ-FLX4 (and the rest of the Pioneer / AlphaTheta range), as a native Linux app: one program, no browser, no server. It does everything the web version does: keylock, RGB waveforms that follow tempo, beat grids you can edit, Camelot keys, phrase detection, hot cues and loops, sync, playlists, rekordbox USB sticks and rekordbox XML.

Written in Rust (egui for the window, ALSA for sound and MIDI, which also reaches PipeWire and PulseAudio through their ALSA plugins). Built for **x86-64-v3** CPUs (Intel Haswell 2013+, AMD Zen 1+).

## Install

```sh
tar xzf cratebox-0.1.0-linux-x86_64-v3.tar.gz
cd cratebox-0.1.0-linux-x86_64-v3
./install.sh
```

That puts `cratebox` in `~/.local/bin` and Cratebox in your app menu; no root needed. `./install.sh --remove` takes it out again (your library data stays).

Needs: an x86-64-v3 CPU, glibc 2.30 or newer (Ubuntu 20.04+, Debian 11+, Fedora 32+, Arch, Mint 20+...), ALSA (`libasound2`, on every desktop distro) and OpenGL (any graphics driver from the last decade). Works on X11 and Wayland.

## First run

1. Plug in the FLX4 (USB). Cratebox finds it by name and picks its mapping; the pill at the top turns green.
2. **Add music folder** and pick the folder you DJ from. Cratebox reads it in place, nothing is copied. New tracks are analysed in the background (BPM, grid, key, phrases); results are kept.
3. Pick the sound output in **Settings → Sound** (it chooses the FLX4 by itself if it can).

You can also drop audio files or folders onto the window. Drop a file on a deck to load it there.

### Sound on Linux, and headphone cueing

The FLX4's sound card has four outputs: 1–2 go to the speakers (MASTER), 3–4 to its headphone jack. Cratebox puts the master on 1–2 and your cue mix on 3–4 when it can open all four (**Headphones: Auto**). The pill at the top shows how many channels it got.

- **Best:** in Settings → Sound → Output, pick the FLX4's own device, `hw:CARD=DDJFLX4,DEV=0` (shown as `DDJFLX4 (hw)`). That gives all four channels with the lowest latency.
- **If that device is busy:** PipeWire or PulseAudio is holding the FLX4. Either set the FLX4's profile to *Off* in your sound settings (pavucontrol → Configuration, or GNOME/KDE sound settings) so Cratebox can have it, or set it to *Pro Audio* and pick it through the `pipewire` device.
- **No FLX4 sound card?** Use **Split** (master in the left ear, cue in the right, with a splitter cable) or **Another device** (headphones on a second sound card).
- **Test cue** in the header beeps in the headphone mix only: if you hear it on the speakers, cueing isn't separated yet.
- **After sleep or a lost device:** if the output stops being called for a second (for example PipeWire restarting after a wake), Cratebox closes it and reopens the same device every 2 seconds until it works. The status line says "Reconnecting…" and then "Sound reconnected".
- **Realtime:** the audio thread asks for realtime priority (SCHED_FIFO 70), so the sound keeps going while the window or the analysis is busy. That needs an rtprio limit of 70 or more for your user (Cratebox OS sets one; elsewhere add e.g. `@audio - rtprio 70` to `/etc/security/limits.d/` and log in again). Without it Cratebox says so on the terminal and runs at normal priority.
- **Audio buffer** (Settings → Sound): 256 samples (about 5 ms) is the default. Go lower for a tighter jog feel, higher if you hear crackles.

If no sound card can be opened, Cratebox keeps running silently (the decks still move) and tells you in the status line.

### Controllers on Linux

Controllers are reached through the ALSA sequencer, which every desktop distro sets up. If no controller ever shows up, run `sudo modprobe snd-seq`. Cratebox rescans every couple of seconds, so a controller plugged in later is picked up.

| Hardware | Mapping |
|---|---|
| DDJ-FLX4 | Built in, checked on the hardware (table below) |
| DDJ-400, DDJ-200, DDJ-SB/SB2/SB3, DDJ-SX/SX2/SX3, XDJ-RX, CDJ-2000/NXS/NXS2, CDJ-850, CDJ-350 | From Mixxx's community mapping for that model |
| Other rekordbox-series units (FLX2, FLX6, FLX10, 800, 1000, GRV6, RB, RR, RX, RZ, XDJ-RX2/RX3/XZ/AZ, OPUS-QUAD) | The shared rekordbox-series layout |
| Other Serato-series units (SR, SR2, SZ, SZ2, REV1/5/7, 1000SRT) | The shared Serato-series layout |
| CDJ-3000 and other players in MIDI mode | The CDJ-2000 layout |
| Anything else that speaks MIDI | **Learn** |

**Learn:** Settings → Controller → **Learn…**, click a control on screen (yellow outline), then press, move or turn the real one. For the jog, touch the platter then turn it. Library controls (browse knob, load, jog side) are in the yellow bar. Learned controls are saved per mapping; **Export** / **Import** move them to another computer.

### DDJ-FLX4 controls

| Control | What it does |
|---|---|
| PLAY/PAUSE | Play / pause. SHIFT: play from the cue point |
| CUE | CDJ-style: playing → back to the cue point; stopped → set the cue (on the beat with Q on); on the cue → preview while held. SHIFT: back to the start |
| BEAT SYNC | Match tempo and beat phase to the other deck; stays locked. Long press: sync on. SHIFT: tempo range ±6/10/16%/WIDE |
| Jog top (touched) | Scratch. Fling it backwards and let go for a backspin (length in Settings) |
| Jog side | Nudge while playing, fine search while stopped. SHIFT + jog: fast search |
| TEMPO | Top is slower, like a CDJ |
| LOOP IN / OUT / RELOOP-EXIT | Manual loops; hold IN half a second for a 4-beat loop. SHIFT+IN halves, SHIFT+OUT doubles |
| CUE/LOOP CALL ◀ ▶ | In a loop: halve/double. Otherwise: previous/next hot cue |
| Pads: HOT CUE / BEAT LOOP / BEAT JUMP | Hot cues A–H (SHIFT+pad deletes) / 1/4–32-beat loops / jump ±1, 2, 4, 8 beats |
| Faders, crossfader, TRIM, EQ (full kill), CFX filter, headphone CUE | Mixer |
| BROWSE / press BROWSE / LOAD 1, 2 | Scroll / jump between the playlist tree and the tracks / load |
| SHIFT + BROWSE turn / press | Waveform zoom / expand the library |

LEDs follow the software (PLAY and CUE blink like a CDJ, pads show hot cues and loop sizes, the channel meters show levels).

## Using it

- **Load:** double-click a track, drag it onto a deck, press LOAD, or Shift+← / Shift+→.
- **Waveforms:** drag to move through the track (while playing, it scratches), click to jump, mouse wheel to zoom. They squeeze with tempo, so synced decks show beats at the same spacing. RGB colours: red lows, green mids, blue highs (3-band style in Settings).
- **Overview** under each deck title: click or drag to jump; the coloured strip is the phrases.
- **Jog wheel** on screen: grab and turn it to scratch.
- **MT** = master tempo (keylock), **Q** = quantize, **GRID** = grid editor (type a BPM, ±0.01, ÷2/×2, TAP, slide 5 ms, BEAT HERE, BAR 1 HERE, RESET). Hand-edited BPMs show yellow in the library and are never overwritten.
- **Pads:** left-click uses, right-click (or Shift+click) deletes a hot cue.
- **Keys** are Camelot (8A = A minor). While a deck plays, keys that mix with it are highlighted; type `8a` in search to filter by key.
- **Phrases:** Intro, Build, Drop, Groove, Breakdown, Outro, shown on the waveforms; the deck shows e.g. `DROP 13/16`.

### Playlists

The tree on the left: **Collection**, your **Playlists**, and rekordbox **Devices**.

- **+** new playlist, **▤** new folder (type a name, Enter). Double-click to rename, hover and click **×** to delete.
- Add tracks by dragging rows onto a playlist, with **Add to ▾**, or right-click → Add to playlist. Ctrl/Shift-click selects several; Ctrl+A selects all.
- Inside a playlist, drag rows to rearrange; **Delete** removes from the playlist (never from disk). Click **#** to go back to the playlist's order after sorting.
- Drag a playlist onto a folder to file it; onto the PLAYLISTS heading to take it back out.

### rekordbox

- **USB sticks / SD cards:** **+ USB** under DEVICES (or Settings → rekordbox), then pick the drive itself (your file manager mounts it under `/media/you/` or `/run/media/you/`; those are listed as buttons). Read-only: every track, the playlist tree, and rekordbox's own beat grid, phrases, hot cues and memory cue for each track you load. **+** on a device playlist copies it into yours. Dropping a drive's folder on the window works too.
- **XML:** the **⋯** next to PLAYLISTS. *Import*: in rekordbox, File → Export Collection in xml format, then pick that file; playlists arrive in a "rekordbox (date)" folder and matching tracks take rekordbox's grid, key and cues (your own edits win). *Export*: writes every track with grid, key and cues, and all your playlists; in rekordbox, Preferences → Advanced → Imported Library = that file, and tick rekordbox xml under View → Layout. To use the XML on another computer, set "Export paths as if the music folders were in" (Settings → rekordbox).

## Where things are kept

`~/.local/share/cratebox/` (or `$XDG_DATA_HOME/cratebox`): `settings.json`, `analysis.json` (the analysis cache), `cues.json`, `playlists.json`. Plain JSON, safe to back up or copy to another machine. Set `CRATEBOX_DATA` to use another folder.

## Build from source

```sh
# Debian/Ubuntu: sudo apt install build-essential pkg-config libasound2-dev
# Fedora: sudo dnf install gcc pkgconf alsa-lib-devel      Arch: sudo pacman -S base-devel alsa-lib
cargo build --release          # x86-64-v3, set in .cargo/config.toml
./target/x86_64-unknown-linux-gnu/release/cratebox
```

For an older CPU, build for it: `cargo build --release --config 'build.rustflags=[]'`. `./build-release.sh` makes the distributable tarball (glibc 2.31 baseline, via cargo-zigbuild).

## Tests

`cargo test --release` runs:
- unit tests: FFT, keys, the deck player (keylock pitch, varispeed, loops, backspin), the mixer (EQ, routing, limiter), MIDI profiles and learn, rekordbox readers on a real USB export, playlists, XML round trip, the resampler;
- `tests/analysis.rs`: BPM, grid, key and phrase accuracy on synthetic tracks with known answers;
- `tests/app.rs`: the whole app with FLX4 MIDI messages: load, play timing, sync and phase, cues, loops, grid edits, playlists, rekordbox XML and USB, learn, and everything coming back after a restart;
- `tests/ui.rs`: the real window under Xvfb, driven with real clicks, drags and keys (needs `xvfb-run`; skipped otherwise).

The synthetic test tracks come from `tests/make_tracks.py` (`python3 tests/make_tracks.py tests/music`). The USB fixture is from pyrekordbox's test data (MIT licence).

## Credits and licences

Mixxx-derived controller mappings: generated from the [Mixxx](https://mixxx.org) project's controller mappings (GPL-2.0). rekordbox file formats: Deep Symmetry's crate-digger documentation. Fonts: Inter (SIL Open Font License) and DejaVu (free licence), see `licenses/`.
