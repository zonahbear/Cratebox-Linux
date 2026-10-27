# What's new in Cratebox OS

Newest first. Each update arrives through `cratebox-update` (no new ISO needed) unless it says "Cratebox OS".

## 27 October 2026 (evening)

- SOULSEEK button (magnifying glass) in the header opens Nicotine+, saving straight into the collection.
- Soulseek, Chrome and the file manager open as normal windows over Cratebox: drag the title bar to
  move, drag an edge to resize, double-click the title to maximise, Alt+Tab to switch.
- Settings has tabs with icons down the left: Look, Library, Decks, Sound, Controller, Wi-Fi,
  Bluetooth, rekordbox.
- Wi-Fi and Bluetooth inside Cratebox: switch them on or off, scan, join a network (with its password),
  pair, connect, disconnect and forget devices. Click the Wi-Fi or Bluetooth readout in the header to
  jump there.
- Fixed a stray vertical line through the WEB (globe) icon.
- The scrolling waveforms are drawn once into pictures kept on the graphics chip and slid by fractions
  of a pixel every frame: no more stepping 4, 5, 4 pixels a frame, no flickering bars, and less work
  for the MacBook each frame.
- Scrolling waveforms move by an exact amount every screen refresh, so they glide instead of shuffling,
  and they no longer wobble when the sound card's reported delay changes.
- Settings → Look → "Show frame rate in the header": shows frames per second and missed screen
  refreshes (60 FPS · 0 MISSED is smooth), so a stutter can be pinned on the screen or on Cratebox.

## 27 October 2026

**Smoother while you play**
- Saving the library and settings happens in the background, so a big collection no longer makes the
  screen hiccup once a second.

**Look and feel**
- Five colour themes: Night (the usual), Midnight, Ember, Neon and Mono. Settings → Look.
- The FILES, WEB, LIB and WAVE buttons in the header are icons now, and lots of labels say what they do
  in plain words ("Remove from playlist", "Jump ahead 4 beats", "NO SOUND"…).
- Power button in the far right corner: shut down or restart cleanly (decks are unloaded and the stick
  is ejected first).

**Battery**
- At 5% a reminder pops up (OK to dismiss).
- At 3% the music stops and the USB stick is ejected safely, then you choose "I've plugged in" or
  "Shut down now".

**Library**
- Quick sort buttons next to the search box: BPM, KEY, and MATCH (tracks that mix into what's playing
  come first: same key, then neighbouring keys, then the closest tempo, half and double time included).
- Keys that mix with the playing deck are highlighted more clearly.
- New PLAYLISTS column shows which playlists each track is in.
- Loading a track from BROWSE takes you straight back to the decks (Settings can turn that off).
- Each deck has a load lock (the padlock) so nothing gets loaded onto it by accident.

**Beatgrids**
- Command+Shift+X sets bar 1 at the playhead on the left deck, Command+Shift+. on the right deck.
- Tracks in your collection that are also on a rekordbox stick use rekordbox's beatgrid.
- Settings → rekordbox → Compare grids writes how far each Cratebox grid is from the stick's
  (grid-compare.log), to track down grids that start half a bar late.

**Waveforms**
- The scrolling waveforms allow for the sound card's delay, so what you see lines up with what you hear.
  A "Waveform timing" slider in Settings fine-tunes it.

**HEAT**
- GLOW: the Puffco's light drifts slowly through the rainbow and brightens with the music. The FX
  LEVEL/DEPTH knob can set its brightness, speed or sensitivity. It never changes the light during a
  heat run or HOLD. (Not yet tried on a real Proxy.)

**Web**
- The FIREFOX button is now WEB: Chrome in its own profile, quick to open, with downloads going straight
  into Music/Cratebox Downloads (and so into the collection).

## 26 October 2026

**Decks and mixer**
- Four decks with layer buttons (1⇄3, 2⇄4), or three decks (1 and 3 share the left side). The DECKS
  button cycles 2, 3, 4.
- XDJ-style deck screens; screens only while a controller is connected, with SYNC, MT, Q, TAP, GRID, the
  title and the whole-track waveform.
- Beat FX and Pad FX, and REC (arms until a deck plays).
- Hold SYNC (or right-click it) to make a deck the master. Master and trim dB readouts; the FLX4's master
  level knob works.
- The crossfader sits dead centre, and the FLX4's middle position snaps to exactly 50%.
- The selected deck strobes; WAVE mode shows just the waveforms, with a pop-up library.

**Library and USB**
- rekordbox sticks mount and open by themselves; COPY USB and EJECT next to BROWSE. A plugged-in stick
  syncs new and changed files to this computer (it never deletes anything).
- Downloads land in Music/Cratebox Downloads, join the collection and copy to the stick.
- BPM range 70-180, or a custom range; the library divider resizes the waveforms, not the decks.

**Header**
- Battery, CPU, memory, network up/down, Wi-Fi signal and Bluetooth.
- LIB toggle, FILES button, OFF menu, F11 full screen.

**HEAT**
- Pairs the Puffco by itself with clearer errors, and reconnects after a dropped link.
- PUFF is a theremin: its voice glides up to two octaves with the hit, over the tape warble.

**Cratebox OS**
- Built on Lubuntu 26.04 around the MacBook's verified speaker setup; sound comes back by itself after
  sleep.
- Cratebox boot splash, Claude desktop app, minimal install that keeps Cratebox, faster boot.
- Updates to the app arrive with `cratebox-update`, no new ISO.
- The audio thread can no longer get Cratebox killed when it falls behind.

## 15 October 2026

- First version: Cratebox for Linux, a native port of the Cratebox web app (same analysis, same
  controller maps, DDJ-FLX4 first).
