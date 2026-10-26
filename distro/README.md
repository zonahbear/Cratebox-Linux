# Cratebox OS

A Lubuntu 26.04 LTS live/install ISO that boots straight into Cratebox, full screen. Audio comes
first, and it is built for the 12-inch MacBook (model A1534, MacBook10,1 / 2017; MacBook9,1 should
behave the same). It also runs on other x86-64-v3 PCs. **Nicotine+** (Soulseek), **Google Chrome** and
the **Claude desktop app** (Anthropic's official Linux beta) are on it too. It boots with a Cratebox
splash.

The MacBook settings come from what was tested on Jonah's MacBook10,1 (Lubuntu 26.04, kernel
7.0.0-30). The table below says which parts were **verified** on that machine and which are only
**proposed**.

## Get it

GitHub → Actions → **cratebox-os** → Run workflow (about an hour). The ISO goes to the `kiosk-latest`
release in parts, because a release file can't be over 2 GiB:

```sh
cat cratebox-os.iso.part-* > cratebox-os.iso
sha256sum -c cratebox-os.iso.sha256
```

Write it to a USB stick (balenaEtcher, or `sudo dd if=cratebox-os.iso of=/dev/sdX bs=4M status=progress`).
On the MacBook, hold **Option (⌥)** at power-on and pick the orange **EFI Boot** drive.

Local build (root, about 25 GB free): `sudo distro/build-iso.sh lubuntu-26.04.1-desktop-amd64.iso target/x86_64-unknown-linux-gnu/release/cratebox out/cratebox-os.iso`.

## First boot on a MacBook: set StartupMute once (speakers)

The speakers stay silent unless the Mac's firmware switches the amp on at power-up, and it only does
that when the NVRAM variable `StartupMute` is `%00` (boot chime on). This setting lives in the Mac's
NVRAM, not on the disk, so the ISO can't carry it. Set it once per machine, and again after an NVRAM
reset (Option+Cmd+P+R erases it). This was verified on Jonah's machine.

1. Hold **Cmd+Option+R** at power-on (Internet Recovery) → Utilities → Terminal.
2. `nvram StartupMute=%00 ; nvram StartupMute` should print `StartupMute	%00`.
3. Restart and boot the stick.

From Linux there is an untested fallback:
`printf '\x07\x00\x00\x00\x00' | sudo tee /sys/firmware/efi/efivars/StartupMute-7c436110-ab2a-4bbb-a880-fe41995c9f82 >/dev/null`.

## Using it

- **Boot:** straight into Cratebox, full screen (`cratebox --kiosk` under a bare openbox, with no panel).
- **Desktop (Soulseek, Chrome, Wi-Fi):** quit Cratebox with **Alt+F4** or **Ctrl+Alt+Q** and the
  Lubuntu desktop starts in the same session. On the live stick you get the normal "Try / Install"
  prompt. To get back to Cratebox, use its menu entry (opens in a window) or reboot.
- **Install:** from the desktop, choose Install. **Normal** or **Minimal** both give you everything
  Cratebox OS adds; Minimal then takes out Lubuntu's extra apps (office, media players and so on) at
  the end of the install. The installer turns on automatic login, so every boot lands in the Cratebox
  session. The login screen also lists *Lubuntu*.
- **Claude:** in the desktop menu. Sign in with your Claude account. Updates come with the system's
  apt updates (Anthropic's repo is registered). Cowork needs hardware virtualization on (it is on a
  MacBook); users are put in the `kvm` group for it.
- **Updating Cratebox:** menu → **Update Cratebox** downloads the newest app from the `app-latest`
  release (GitHub → Actions → **cratebox-app**, a few minutes), checks it and replaces
  `/usr/bin/cratebox` (the old one stays as `cratebox.previous`). The repo is private, so the first
  run asks for a GitHub fine-grained token with read access to it; it is kept in
  `~/.config/cratebox/github-token`. No new ISO needed for app changes.
- **Boot menu:** the stick waits 3 s on its menu; the installed system doesn't show one (hold **Esc**
  at power-on to get it, e.g. for an older kernel).
- **Sleep:** s2idle only (see below). Closing the lid is fine, but shut down before a long stretch
  on battery, because s2idle drains more than deep sleep.

## What's changed from stock Lubuntu 26.04.1

| Change | Status on the MacBook10,1 |
|---|---|
| Kernel stays at the ISO's 7.0.0-30 and is **held** (`apt-mark hold`). `linux-headers` and `linux-source` 7.0.0-30 are added for the driver build | Verified kernel. The hold is new: unhold it on purpose, then test speakers |
| [macbook12-audio-driver](https://github.com/leifliddy/macbook12-audio-driver) 0.1, unpatched, DKMS, at commit 4cdfcdb. The build stops if its `patch_cirrus` files differ from the verified ones (sha256 4f733852 / 3f21e687 / cc77fb60) | Verified |
| WirePlumber rule `api.alsa.soft-mixer = true` for `alsa_card.pci-0000_00_1f.3`, system-wide in `/etc/wireplumber/wireplumber.conf.d/` | Rule verified. The system-wide location is untested (it was tested in `~/.config`) |
| Boot service sets ALSA Master and PCM to 100%, on MacBook9,1/10,1 only | Settings verified. Applying them at every boot is new |
| `mem_sleep_default=s2idle` (deep sleep kills the speakers until the next power-up) | Verified |
| udev rule: no D3cold for the Apple NVMe at `0000:01:00.0` (otherwise sleep never wakes) | Verified |
| `system-sleep/50-restart-pipewire`: after resume, restarts PipeWire for every logged-in user. Cratebox notices its output went dead and reconnects by itself (apps that don't do this must be restarted, as the brief found) | Manual restart verified. This any-user hook is not. Before the reconnect fix, Cratebox stayed silent after a wake (Jonah, 2026-10-26) |
| Build fails if TLP, a `power_save` modprobe option or the old audio debug modprobe file shows up. pipewire, pipewire-pulse, wireplumber and rtkit are pinned as manually installed | Follows the brief's rules |
| `applespi` + SPI modules in the dracut initramfs | Proposed. Keyboard/trackpad use the mainline `applespi` driver |

**Against dropouts and stutter** (all proposed: sensible, but not measured on the machine):

| Change | Why |
|---|---|
| Cratebox's audio thread runs **realtime** (SCHED_FIFO 70, under PipeWire's own threads). Login users join the `cratebox-rt` group at boot, which `/etc/security/limits.d/30-cratebox-rt.conf` allows rtprio 70. No CPU time limit is set, so an overloaded moment can stutter but can't kill Cratebox | Otherwise a busy UI or analysis thread could delay it. PipeWire's own setup is untouched |
| Cratebox buffer **512 frames** for new users (about 10 ms per period at 48 kHz). Settings → Sound changes it | The default 256 is tight for a 4.5 W fanless Core m |
| PipeWire `default.clock.min-quantum = 256` | Stops apps such as Chrome pulling the graph below 5 ms |
| `preempt=full` | The generic kernel is PREEMPT_DYNAMIC; full preemption gives the audio threads steadier scheduling |
| **performance** power profile while Cratebox is open, restored when it closes. thermald stays on | The CPU ramps up fast for load bursts. HDA power saving is left exactly as stock (brief rule) |
| Kiosk session runs openbox only, with no panel, compositor or applets; screen blanking is off | Less running next to the audio |
| No snapd, Firefox snap or LibreOffice; Chrome is the one browser | Lighter |
| `threadirqs` (interrupt handlers as threads, as Ubuntu Studio does) and `nowatchdog` | IRQ work can be scheduled around the audio threads; no NMI watchdog ticking |
| No automatic apt runs (`apt-daily*` timers off), no CUPS, ModemManager, kerneloops, whoopsie or apport | Nothing kicks off in the background mid-set. Update by hand: `sudo apt update && sudo apt upgrade` |
| Intel VA-API driver (`intel-media-va-driver-non-free`) | Chrome and Claude decode video on the GPU, not the 4.5 W CPU (less heat, less throttling) |
| zram swap (half the RAM, zstd), swappiness left at the default | Memory pressure swaps to RAM, not the SSD. A higher swappiness would page out decoded tracks |

**Faster boot** (proposed): the stick's boot menu waits 3 s instead of 30; the installed system's
GRUB menu is hidden with no timeout, including after an unclean shutdown (`GRUB_RECORDFAIL_TIMEOUT=0`);
NetworkManager-wait-online is off, so the session doesn't wait for Wi-Fi.

**Bluetooth for HEAT:** Bluetooth works with the stock drivers (Jonah). `JustWorksRepairing = always`
in `/etc/bluetooth/main.conf`, so a Puffco that dropped its bond pairs again without a prompt, and
users are in the `bluetooth` group.

Wi-Fi (Broadcom BCM4350, `brcmfmac`) uses Ubuntu's own firmware. The display runs at 2x (DPI 192,
`GDK_SCALE=2`, `GDK_DPI_SCALE=0.5`) on the 2304×1440 panel, which gives Cratebox 1152×720 points.

## Check after installing (from the brief's checklist)

```sh
cat /sys/power/mem_sleep                                   # [s2idle] deep
cat /sys/bus/pci/devices/0000:01:00.0/d3cold_allowed       # 0
modinfo -n snd_hda_codec_cs420x                            # .../updates/dkms/...
amixer -c 0 get Master                                     # 100%, and stays there while you move the volume
ps -eLo cls,rtprio,pid,comm | grep -w FF                    # with Cratebox open: one of its threads at 70
```

Then play through the speakers after a **warm reboot** and after a **cold boot**, test headphones
(play, plug/unplug), and test speakers after a sleep/wake. Do all of this before trusting it at a gig.

## Not done

- FaceTime HD camera driver and hibernation are left out, to keep this scoped.
- Not done for boot time: a host-only or lz4 initramfs. It would load faster, but a wrong host-only
  guess leaves the machine unbootable, so it waits for a timed boot on the MacBook first.
- Kept on purpose: CPU vulnerability mitigations (turning them off is faster on Kaby Lake but not
  safe on a machine that browses), and the i915 GuC/HuC firmware defaults.
- MacBook8,1 (2015) isn't supported by the speaker driver.
- The ISO contains Chrome and the Claude desktop app, whose licences don't allow passing them around, so
  keep the release private.

## Updating the kernel later

```sh
sudo apt-mark unhold linux-image-7.0.0-30-generic linux-modules-7.0.0-30-generic linux-headers-7.0.0-30-generic linux-source-7.0.0 linux-generic linux-image-generic linux-headers-generic
sudo apt full-upgrade          # DKMS rebuilds the speaker driver for the new kernel
```

Then reboot and run the checks above again. If the speakers are silent, boot the previous kernel
from the GRUB menu (Advanced options).
