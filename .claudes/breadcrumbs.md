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
