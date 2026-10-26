# CLAUDE.md: Cratebox for Linux

Read this first. Update it when structure, conventions or open work change. Keep it short.

## What this repo is

Native Rust/egui port of Cratebox (two-deck DJ software, DDJ-FLX4 first). Built for x86-64-v3. README.md is
the user guide; this file is the map.

## Map

```
src/main.rs        entry: CPU check, display env fixes, window (--kiosk = full screen, no decorations)
src/               lib (ui.rs = egui Gui, audio/MIDI/analysis/rekordbox modules)
tests/             analysis.rs, app.rs (FLX4 MIDI end to end), ui.rs (Xvfb); fixtures/rekordbox_usb
packaging/         install.sh, cratebox.desktop, icon; build-release.sh makes the glibc-2.31 tarball
distro/            Cratebox OS ISO: build-iso.sh remasters Lubuntu 26.04.1 (layered squashfs: chroot-setup.sh runs in
                   the minimal.standard layer, live-setup.sh in the live layer), overlay/ = files for the installed
                   system, live/ = live-session script. distro/README.md = user guide + verified/proposed table
.github/workflows/cratebox-os.yml  manual-only ubuntu job → release "kiosk-latest" (ISO split into <2 GiB parts)
```

## Conventions

- Tests: `cargo test --release` (ui.rs needs xvfb-run). `env.sh` is only for the original build workspace.
- Cratebox OS is audio first, for Jonah's MacBook10,1: keep the ISO's kernel (7.0.0-30, verified with the speaker
  driver) and hold it; s2idle only; never TLP or HDA power_save tweaks; never remove pipewire/wireplumber/rtkit.
  StartupMute=%00 is per-machine NVRAM (first-boot step in distro/README.md), the image can't set it.
- Build deps: libasound2-dev, libdbus-1-dev (cpal's audio_thread_priority = realtime audio thread via rtkit).
- Actions minutes are scarce: workflows stay manual (`workflow_dispatch`).

## Open work

- Cratebox OS: the remaster ran in a cloud session without the driver and Chrome (SKIP_THIRD_PARTY=1); the full
  build is Actions → cratebox-os. Then on the MacBook: the checks in distro/README.md (speakers after warm reboot,
  cold boot and sleep; headphones). The "proposed" anti-dropout settings there are unmeasured.
- Not yet tried on real sound hardware or a controller (README).
