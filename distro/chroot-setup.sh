#!/bin/bash
# Runs inside the installed-system layer of Lubuntu 26.04 (build-iso.sh stages this repo's bits in
# /tmp/cratebox-os). Turns it into Cratebox OS for the 12-inch MacBook (A1534, MacBook10,1).
# Audio comes first: this follows what was verified on Jonah's MacBook10,1 (distro/README.md lists
# which parts were verified on the machine and which are only proposed).
set -euxo pipefail
export DEBIAN_FRONTEND=noninteractive
src=/tmp/cratebox-os
apt_get() { apt-get -y -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold "$@"; }

# the ISO's own apt source (file:/cdrom) only exists on the stick: off during the build, put back after
cdrom=/etc/apt/sources.list.d/cdrom.sources
if [ -e "$cdrom" ]; then mv "$cdrom" "$src/"; trap 'mv "$src/cdrom.sources" "$cdrom"' EXIT; fi
apt-get update

# Kernel: keep exactly the one the ISO ships (7.0.0-30, the kernel the speaker driver was verified
# on) and hold it, so an update can't swap the kernel under the out-of-tree driver before a gig
kver=$(ls /lib/modules)
[ "$(echo "$kver" | wc -l)" = 1 ] || { echo "expected one kernel, got: $kver" >&2; exit 1; }
kpkgver=$(dpkg-query -W -f '${Version}' "linux-image-$kver")
apt_get install --no-install-recommends "linux-headers-$kver" "linux-source-${kver%%-*}=$kpkgver" dkms build-essential
apt-mark hold "linux-image-$kver" "linux-modules-$kver" "linux-headers-$kver" "linux-source-${kver%%-*}" \
    $(dpkg-query -W -f '${Package}\n' | grep -E '^linux-(generic|image-generic|headers-generic|modules-extra-'"$kver"')$')

# Speaker driver for MacBook9,1 / MacBook10,1 (leifliddy/macbook12-audio-driver), unpatched, DKMS
if [ -z "${SKIP_THIRD_PARTY:-}" ]; then
    drv=/usr/src/macbook12-audio-driver-0.1
    rm -rf "$drv" && install -d "$drv"
    cp -a "$src"/macbook12-audio-driver/{dkms.conf,Makefile_cirrus,Makefile_cs420x,prepare.cirrus.driver.sh,patch_cirrus} "$drv"/
    (cd "$drv/patch_cirrus" && sha256sum cs420x.c patch_cirrus_a1534_pcm.h patch_cirrus_a1534_setup.h) | cut -c1-8 | tr '\n' ' ' \
        | grep -qx '4f733852 3f21e687 cc77fb60 ' || { echo "driver sources differ from the verified ones" >&2; exit 1; }
    dkms add -m macbook12-audio-driver -v 0.1
    dkms install -m macbook12-audio-driver -v 0.1 -k "$kver"
    modinfo -k "$kver" -n snd_hda_codec_cs420x | grep -q /updates/
fi

# Apps: Nicotine+ (Soulseek), Google Chrome (the one browser), and what the kiosk session uses
apt_get install nicotine openbox x11-xserver-utils alsa-utils power-profiles-daemon
if [ -z "${SKIP_THIRD_PARTY:-}" ]; then
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /etc/apt/keyrings/google-chrome.gpg
    echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/google-chrome.gpg] https://dl.google.com/linux/chrome/deb/ stable main" \
        > /etc/apt/sources.list.d/google-chrome.list
    apt-get update
    apt_get install google-chrome-stable
fi

# Lighter: no snapd / Firefox snap (Chrome is the browser), no office suite on a DJ machine
apt-mark manual pipewire pipewire-pulse wireplumber rtkit
apt_get purge snapd firefox $(dpkg-query -W -f '${Package}\n' | grep '^libreoffice')
apt_get autoremove --purge

# Cratebox
install -m 0755 "$src/cratebox" /usr/bin/cratebox
install -Dm 0644 "$src/cratebox.desktop" /usr/share/applications/cratebox.desktop
install -Dm 0644 "$src/cratebox.png" /usr/share/icons/hicolor/512x512/apps/cratebox.png
gtk-update-icon-cache -q /usr/share/icons/hicolor || true
cp -a "$src"/overlay/. /
systemctl enable macbook12-audio-amps.service

# The brief's hard rules, checked rather than trusted: the audio stack is all there, nothing that
# switches on HDA power saving, no debug modprobe file
for p in pipewire pipewire-pulse wireplumber rtkit; do dpkg-query -W -f '${Status}\n' "$p" | grep -q 'install ok installed'; done
fail() { echo "$1" >&2; exit 1; }
if dpkg-query -W -f '${Status}\n' tlp 2>/dev/null | grep -q 'install ok installed'; then fail "tlp is installed"; fi
if grep -rqs 'power_save' /etc/modprobe.d/ /usr/lib/modprobe.d/; then fail "a modprobe file sets power_save"; fi
if [ -e /etc/modprobe.d/99-macbook12-audio-diagnostic.conf ]; then fail "the audio debug modprobe file is there"; fi

apt-get clean
