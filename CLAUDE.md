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
distro/            kiosk ISO: build-iso.sh (remasters Lubuntu 24.04.5), chroot-setup.sh, overlay/ (session,
                   openbox rc, MacBook audio bits). See distro/README.md
.github/workflows/kiosk-iso.yml   manual-only ubuntu job → release "kiosk-latest" (ISO split into <2 GiB parts)
```

## Conventions

- Tests: `cargo test --release` (ui.rs needs xvfb-run). `env.sh` is only for the original build workspace.
- Kiosk ISO keeps the GA 6.8 kernel: the 12-inch MacBook speaker driver can't build for HWE kernels on Ubuntu.
- Actions minutes are scarce: workflows stay manual (`workflow_dispatch`).

## Open work

- Kiosk ISO has never been built or booted (the cloud sandbox would not run the driver build as root). First real run:
  Actions → kiosk-iso, then boot on Jonah's MacBook10,1 and check speakers, keyboard/trackpad, Wi-Fi.
- Not yet tried on real sound hardware or a controller (README).
