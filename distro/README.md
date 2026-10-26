# Cratebox kiosk ISO

A Lubuntu 24.04 LTS live/install ISO that boots straight into Cratebox, full screen. Made for the
12-inch MacBook (model A1534, 2016/2017: MacBook9,1 / MacBook10,1), and works on other x86-64-v3 PCs.
Also on it: **Nicotine+** (Soulseek) and **Google Chrome**.

## Get it

GitHub → Actions → **kiosk-iso** → Run workflow (about an hour). The ISO goes to the
`kiosk-latest` release in parts, because a release file can't be over 2 GiB:

```sh
cat cratebox-kiosk.iso.part-* > cratebox-kiosk.iso
sha256sum -c cratebox-kiosk.iso.sha256
```

Write it to a USB stick (balenaEtcher, or `sudo dd if=cratebox-kiosk.iso of=/dev/sdX bs=4M status=progress`).
On the MacBook, hold **Option (⌥)** at the chime and pick the orange **EFI Boot** drive.

Local build (root, about 25 GB free): `sudo distro/build-iso.sh lubuntu-24.04.5-desktop-amd64.iso target/x86_64-unknown-linux-gnu/release/cratebox out/cratebox-kiosk.iso`.

## Using it

- **Boot:** straight into Cratebox, full screen (`cratebox --kiosk` under a bare openbox, no panel).
- **Desktop (Soulseek, Chrome, Wi-Fi):** quit Cratebox with **Alt+F4** or **Ctrl+Alt+Q**. The Lubuntu
  desktop starts in the same session. On the live stick that is the normal "Try / Install Lubuntu" prompt.
  Back to Cratebox: its menu entry (windowed), or reboot.
- **Install:** from the desktop, Install. The installer sets "log in automatically", which brings
  you back to the Cratebox session on every boot. The SDDM login screen also lists *Cratebox* and *Lubuntu*.
- Your library data is in `~/.local/share/cratebox/` as usual. The live session forgets everything at power-off.

## What's changed from stock Lubuntu

| Change | Why |
|---|---|
| GA kernel 6.8 instead of the HWE kernel | The speaker driver builds from Ubuntu's `linux-source` tarball, which only exists for GA kernels |
| [macbook12-audio-driver](https://github.com/leifliddy/macbook12-audio-driver) (pinned commit, DKMS) + WirePlumber soft-volume rule + a boot service that sets the amps to 100% | Stock Linux finds the CS4208 codec but never turns the speaker amp on (MacBook9,1/10,1). Steps from that driver's README |
| `applespi`, `intel_lpss_pci`, `spi_pxa2xx_platform`, `spi_pxa2xx_pci` in the initramfs | Keyboard and trackpad are SPI (mainline `applespi` since Linux 5.3); loading early makes them work from the first screen ([macbook12-spi-driver README](https://github.com/cb22/macbook12-spi-driver/blob/master/README.md)) |
| `preempt=full` | Ubuntu's generic kernel is `PREEMPT_DYNAMIC`; full preemption gives steadier low-latency audio |
| Kiosk session: openbox only, screen blanking off, 2x scale on the 2304×1440 panel | Fewer things running on a 4.5 W Core m; 2x gives Cratebox 1152×720 points (its minimum is 1100×720) |
| No snapd / Firefox snap / LibreOffice; Chrome from Google's apt repo | Lighter, and Chrome is the browser |

Wi-Fi (Broadcom, `brcmfmac`) uses the firmware already in Ubuntu's linux-firmware; nothing added.

## Not done / known gaps

- **Bluetooth** on the 12-inch MacBook needs a patched `hci_uart` ([macbook12-bluetooth-driver](https://github.com/leifliddy/macbook12-bluetooth-driver)); left out.
- **Suspend** on these MacBooks is unreliable on Linux; plan on shutting down instead of closing the lid mid-set.
- The MacBook8,1 (2015) is not supported by the speaker driver.
- Chrome's licence doesn't allow passing the ISO around: keep the release private.
