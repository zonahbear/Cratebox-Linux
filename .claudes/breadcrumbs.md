# Breadcrumbs

## 2026-10-26: kiosk ISO (branch claude/project-thread-nvrvsw)

- Added distro/ (Lubuntu 24.04.5 remaster: GA 6.8 kernel, leifliddy/macbook12-audio-driver via DKMS at
  4cdfcdb, applespi modules in initramfs, Nicotine+, Chrome, kiosk session) and .github/workflows/kiosk-iso.yml.
- `cratebox --kiosk` = fullscreen + no decorations (src/main.rs).
- Checked by hand on the stock ISO: single casper/filesystem.squashfs (gzip), HWE 7.0 kernel, casper writes
  SDDM autologin to lubuntu-live-environment.desktop (Exec /usr/libexec/start-lubuntu-live-env, which we divert),
  brcmfmac43602 firmware present, kernel has CONFIG_KEYBOARD_APPLESPI=m and PREEMPT_DYNAMIC.
- NOT run: the full build (sandbox refused to run the cloned driver scripts as root). Next: run the workflow,
  fix whatever breaks, boot on the MacBook10,1.

## 2026-10-26 (later): Cratebox OS, audio first (Jonah sent the "Audio on macbook" brief)

- Rebased on Lubuntu 26.04.1: it ships 7.0.0-30, the exact kernel the speaker driver was verified on, so
  no kernel swap; it is held. linux-source-7.0.0=7.0.0-30.30 and headers are still in resolute-updates.
- 26.04 ISO is layered (minimal / minimal.standard / minimal.standard.live). build-iso.sh mounts each
  unpacked upper layer as an overlay upperdir again and chroots in, so changes and whiteouts land in that
  layer; minimal, vmlinuz and the live initrd are untouched. Installed system uses dracut (not
  initramfs-tools); Calamares copies /cdrom/casper/vmlinuz into the target, so the kernel must not change.
- Driver commit 4cdfcdb's patch_cirrus files match the brief's known-good sha256 prefixes (checked).
- Brief rules in overlay/: s2idle, NVMe d3cold udev, any-user PipeWire resume hook, WP 0.5 soft-mixer,
  Master+PCM 100% service. Anti-dropout (proposed): cpal audio_thread_priority (needs libdbus-1-dev),
  512-frame default via /etc/skel, PipeWire min-quantum 256, preempt=full, performance profile in kiosk.

## 2026-10-26 (later still): realtime audio thread fix

- QEMU boot of the test ISO: Cratebox died with "CPU time limit exceeded". cpal's `audio_thread_priority`
  (rtkit) sets RLIMIT_RTTIME soft = budget, so an overrunning callback gets SIGXCPU and the whole app dies.
- Fix: feature dropped (no more libdbus); `promote_to_realtime` in src/audio_io.rs sets SCHED_FIFO 70 on the
  first callback, no RTTIME, warns once if it can't. Distro: group cratebox-rt, limits.d rtprio 70,
  cratebox-rt-users.service adds UID 1000-59999 users before the display manager (live user from casper too;
  the live layer has no /etc/group of its own, so the standard layer's group applies). PipeWire untouched.

## 2026-10-26 evening: no sound after sleep/wake (Jonah, on the MacBook)

- Live ISO speakers work on the MacBook10,1 (Jonah). Bluetooth works with stock drivers (Jonah).
- After wake, 50-restart-pipewire restarts PipeWire; Cratebox's cpal stream then sat dead (no error, no
  reopen). Fix: AudioIo counts callbacks per stream (`Beat`); App::watch_output closes + reopens the same
  device when a stream hasn't been called for 1 s, retrying every 2 s. VM check: old binary kept the same
  cpal thread after `systemctl --user restart pipewire ...`; new one reopened (new FIFO-70 thread,
  alsa_playback.cb-new node registered with the new PipeWire).
- 4 decks / wave mode / HEAT / FX / XDJ-like layout went to a separate thread.

## 2026-10-26 night: iOS performance-mode features + XDJ-style screen (branch claude/project-thread-a7yqwi)

- Jonah: 4 decks, WAVE ("just the waveforms showing"), HEAT, FX, XDJ-like screen, smaller buttons, sized for the
  MacBook10,1 (2304×1440 = 1152×720 pt at 2x).
- Done: engine/mixer 4 channels + FX inserts + master FX + MST cue + REC tap; iOS sync port (master pick,
  half/double, phase lock); deck layers + soft takeover; BEAT FX/PAD FX (fx.rs, beatfx.rs); recorder.rs;
  heat.rs (TrueCurve + HOLD/PUFF, simulator, BlueZ via zbus); new ui.rs layout. All tests pass (lib 44,
  app, analysis, four, ui under Xvfb). Screenshots taken at 1152×720 in Xvfb.
- Next: try on the MacBook with the FLX4 (layer buttons, FX, CPU load with 4 decks), and HEAT with the real
  Proxy over BlueZ (pairing without a desktop agent in the kiosk session is the risky bit).
- Same night, Jonah's feedback ("atrocious"): mixer now one column per channel (EQs top-down), 2-deck players
  are the iOS deck (jog + cluster, tall/compact variants in ui.rs), library height + playlist column drag
  (settings lib_h/tree_w), keyboard+trackpad (faders grab-and-slide, scroll nudges, docs/keyboard.md),
  FLX4 sends the Mixxx position query (= keep-alive) at connect.

## 2026-10-26 19:20Z
- PR #7 merged (c53e7bc); cratebox-os run 6 rebuilt kiosk-latest with the 4-deck app. Next: Jonah tests on the MacBook (FLX4, trackpad scroll sign, HEAT over BlueZ).
- 2026-10-26 19:50Z: library divider now trades height with the waveforms (deck row fixed); BPM range 70-180 + Custom (settings bpm_lo/bpm_hi, analysis::custom_range). Same range on iOS #26 and web Cratebox.
- 2026-10-26 20:10Z: XDJ deck screens (deck_screen/deck_display, Settings → Decks auto/screens/buttons), HEAT BlueZ agent + heat.log + clearer connect errors. Jonah: Puffco listed but would not connect on live boot (no error text yet). Distro asks (Claude desktop, plymouth logo, minimal-install, Kaby Lake tweaks) handed to the distro thread.

## 2026-10-26 late: OS batch + app updater
- PR #10 (merged): Claude desktop, Plymouth splash (live initrd gets an appended cpio), Calamares Minimal keeps
  Cratebox (cratebox-minimal-install), threadirqs/nowatchdog/zram/VA-API, services off, fast boot. SIGPIPE in the keep
  loop fixed (awk instead of head). PR #11: `systemctl disable` best-effort (kerneloops stub failed on the runner).
- Updater: cratebox-app workflow → release app-latest; `cratebox-update` (menu "Update Cratebox") installs it with a
  token in ~/.config/cratebox/github-token. Ships in the ISO after the one built from cda9652.
- Next: Jonah tests the ISO on the MacBook (splash, minimal install, Claude sign-in, sleep/wake, HEAT).

## 2026-10-27 app update batch (branch claude/project-thread-nvrvsw)
- OFF menu (exit to login / restart / shut down), F11 full screen, ms beat-lock tag, smoothed waveform clock (deck.vis_now) + pixel-locked lane columns, rekordbox grids kept (grid_manual), USB copy (src/usbcopy.rs, ↓ on stick rows).
- OS files ship via app-latest os-files.tar.gz (cratebox-update installs them): kiosk session exits to SDDM, Xsession.d/45cratebox-hidpi (Cratebox 2x, desktop Xft.dpi 168, Jonah confirmed).
- Open: Alt+F4 landed on a text console on the installed system (SDDM not coming back?), awaiting `systemctl status display-manager`.

## 2026-10-27 03:30Z
- Shipped: header FIREFOX (distro/overlay/usr/bin/cratebox-firefox: firejail --private box, first run installs Mozilla-apt firefox + firejail) and FILES (pcmanfm-qt), Wi-Fi/BT forced on in kiosk session + BlueZ AutoEnable in ISO, selected-deck strobe, LIB toggle, HEAT forget-on-auth-fail.
- Next: HEAT auto-reconnect/longer retries (heat.log shows LE timeouts, bond fine), master+trim readouts, hold SYNC = master, REC arms until play, 3-deck mode, Wi-Fi/BT + net rates in header, PUFF theremin, Soulseek in-app.

## 2026-10-27 03:40Z
- Downloads: ~/Music/Cratebox Downloads (app.rs downloads_dir) is shared by FIREFOX (firejail --whitelist + Firefox policy DownloadDirectory) and Nicotine+ (kiosk session seds downloaddir). Cratebox rescans it on change and copies new files to "<stick>/Cratebox Downloads" (plain files, pdb untouched; Jonah chose this) now and at each plug-in.
- HEAT: 5 connect tries (15 s settle), auto-reconnect after a drop at 2/5/10/20/30 s; user Disconnect/Sim cancels.
- Jonah: in-app Soulseek must scale properly for the 2x display.

## 2026-10-27 WEB button
- FIREFOX → WEB: spawns distro/overlay/usr/bin/cratebox-web (Chrome, own profile ~/.local/share/cratebox/web,
  downloads → ~/Music/Cratebox Downloads, niced, log web.log). cratebox-firefox/firejail removed (too slow mid-set).
- Next: spec list (Beat 1 shortcuts Cmd+Shift+X / Cmd+Shift+., Camelot colours + quick sort, playlist column,
  load closes manager, load lock, battery 5%/3%, graceful shutdown, power icon, themes, Puffco LED), then Soulseek.
