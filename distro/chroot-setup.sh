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

# Claude desktop (official Linux beta, Anthropic's apt repo; https://code.claude.com/docs/en/desktop-linux).
# Its recommends are QEMU/OVMF/virtiofsd for Cowork's VM; gnome-keyring keeps the sign-in.
if [ -z "${SKIP_THIRD_PARTY:-}" ]; then
    key=/usr/share/keyrings/claude-desktop-archive-keyring.asc
    curl -fsSLo "$key" https://downloads.claude.ai/claude-desktop/key.asc
    gpg --show-keys --with-colons "$key" | grep -q '^fpr:::::::::31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE:$' \
        || { echo "Claude desktop signing key has the wrong fingerprint" >&2; exit 1; }
    echo "deb [arch=amd64,arm64 signed-by=$key] https://downloads.claude.ai/claude-desktop/apt/stable stable main" \
        > /etc/apt/sources.list.d/claude-desktop.list
    apt-get update
    apt_get install claude-desktop gnome-keyring
fi

# Kaby Lake (HD 615): hardware video decode, so Chrome and Claude don't decode video on the 4.5 W CPU;
# zram swap (config in overlay/etc/systemd/zram-generator.conf)
apt_get install intel-media-va-driver-non-free systemd-zram-generator

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
groupadd -r -f cratebox-rt
systemctl enable macbook12-audio-amps.service cratebox-rt-users.service

# Boot splash: the Cratebox theme (overlay/usr/share/plymouth/themes/cratebox) plus the stock
# password/keyboard images two-step needs. The installer's dracut run puts it in the initramfs;
# build-iso.sh adds it to the live stick's initrd.
theme=/usr/share/plymouth/themes/cratebox
for f in bullet capslock entry keyboard keymap-render lock; do
    for d in /usr/share/plymouth/themes/spinner /usr/share/plymouth/themes/lubuntu-logo/spinner; do
        [ -e "$d/$f.png" ] && { cp "$d/$f.png" "$theme/"; break; }
    done
done
update-alternatives --install /usr/share/plymouth/themes/default.plymouth default.plymouth "$theme/cratebox.plymouth" 300
update-alternatives --set default.plymouth "$theme/cratebox.plymouth"

# Bluetooth for HEAT (Puffco over BLE): a Puffco that dropped its bond pairs again without a prompt
conf=/etc/bluetooth/main.conf
if grep -q '^#\? *JustWorksRepairing' "$conf"; then sed -i 's/^#\? *JustWorksRepairing *=.*/JustWorksRepairing = always/' "$conf"
else sed -i '/^\[General\]/a JustWorksRepairing = always' "$conf"; fi
grep -qx 'JustWorksRepairing = always' "$conf"
systemctl enable bluetooth.service

# Faster boot: nothing waits for the network before the session starts, and GRUB never stops on
# its menu (Esc at power-on still shows it), not even the 30 s it waits after an unclean shutdown
# masked, not just disabled: the installer copies the layers on top of each other, so a removed wants
# link from the layer below would come back
systemctl mask NetworkManager-wait-online.service systemd-networkd-wait-online.service 2>/dev/null || true

# Nothing in the background mid-set: no automatic apt runs (the kernel is held anyway; update by
# hand), no printer or modem daemons, no crash reporters (proposed)
for u in apt-daily.timer apt-daily-upgrade.timer cups.service cups-browsed.service ModemManager.service \
         kerneloops.service whoopsie.service apport.service; do
    systemctl disable "$u" 2>/dev/null || true    # some exist only as stubs (kerneloops on the runner)
    systemctl mask "$u" 2>/dev/null || true       # see wait-online above
done

# "Minimal installation" (cratebox-minimal-install): the stock full-minus-minimal packages, without
# the ones Cratebox OS itself uses. Those are also marked manual, so autoremove keeps what they need.
keep="nicotine openbox x11-xserver-utils alsa-utils power-profiles-daemon pipewire pipewire-pulse pipewire-alsa
      wireplumber rtkit dkms build-essential intel-media-va-driver-non-free systemd-zram-generator plymouth
      google-chrome-stable claude-desktop gnome-keyring bluez"
for lib in $(ldd /usr/bin/cratebox | awk '$3 ~ /^\// { print $3 }') \
           libxkbcommon-x11.so.0 libxkbcommon.so.0 libEGL.so.1 libGL.so.1 libGLX.so.0 libX11-xcb.so.1 \
           libXcursor.so.1 libXi.so.6 libXrandr.so.2 libwayland-client.so.0 libwayland-egl.so.1 libwayland-cursor.so.0; do
    path=$(ldconfig -p | awk -v l="${lib##*/}" '$1 == l { if (!p) p = $NF } END { print p }')
    [ -n "$path" ] && keep="$keep $(dpkg -S "$(readlink -f "$path")" 2>/dev/null | awk -F: 'NR == 1 { print $1 }')"
done
keep=$(for p in $keep; do dpkg-query -W -f '${db:Status-Abbrev} ${Package}\n' "$p" 2>/dev/null; done | awk '$1 == "ii" { print $2 }' | sort -u)
apt-mark manual $keep >/dev/null
install -d /usr/share/cratebox-os
cut -f1 "$src/minimal.manifest.full.orig" | sed 's/:.*//' | sort -u > /tmp/minimal.pkgs
cut -f1 "$src/minimal.standard.manifest.full.orig" | sed 's/:.*//' | sort -u \
    | comm -23 - /tmp/minimal.pkgs | comm -23 - <(echo "$keep") > /usr/share/cratebox-os/minimal-remove.list
rm /tmp/minimal.pkgs

# The brief's hard rules, checked rather than trusted: the audio stack is all there, nothing that
# switches on HDA power saving, no debug modprobe file
for p in pipewire pipewire-pulse wireplumber rtkit; do dpkg-query -W -f '${Status}\n' "$p" | grep -q 'install ok installed'; done
fail() { echo "$1" >&2; exit 1; }
if dpkg-query -W -f '${Status}\n' tlp 2>/dev/null | grep -q 'install ok installed'; then fail "tlp is installed"; fi
if grep -rqs 'power_save' /etc/modprobe.d/ /usr/lib/modprobe.d/; then fail "a modprobe file sets power_save"; fi
if [ -e /etc/modprobe.d/99-macbook12-audio-diagnostic.conf ]; then fail "the audio debug modprobe file is there"; fi
if ldd /usr/bin/cratebox | grep -q "not found"; then fail "cratebox is missing a library"; fi
if [ "$(readlink -f /usr/share/plymouth/themes/default.plymouth)" != "$theme/cratebox.plymouth" ]; then fail "boot splash isn't the Cratebox theme"; fi
if grep -qx -e pipewire -e wireplumber -e rtkit -e linux-firmware /usr/share/cratebox-os/minimal-remove.list; then fail "minimal install would remove audio or firmware packages"; fi

apt-get clean
