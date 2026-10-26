#!/bin/bash
# Runs inside the unpacked Lubuntu root (build-iso.sh puts this repo's bits in /tmp/cratebox-kiosk).
# Turns stock Lubuntu 24.04 into the Cratebox kiosk for the 12-inch MacBook (A1534).
set -euxo pipefail
export DEBIAN_FRONTEND=noninteractive
src=/tmp/cratebox-kiosk
apt_get() { apt-get -y -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold "$@"; }

apt-get update

# Kernel: the GA 6.8 kernel instead of HWE. The CS4208 speaker driver builds against Ubuntu's
# own linux-source tarball, which only exists for GA kernels (see its prepare.cirrus.driver.sh).
apt_get install --no-install-recommends linux-generic linux-headers-generic linux-source dkms build-essential
apt_get purge $(dpkg-query -W -f '${Package}\n' | grep -E '^linux-.*(hwe|-7\.[0-9]+\.[0-9]+)')
kver=$(ls /lib/modules | sort -V | tail -n1)
case "$kver" in 6.8.*) ;; *) echo "expected the 6.8 GA kernel, got $kver" >&2; exit 1 ;; esac

# Speaker driver for MacBook9,1 / MacBook10,1 (leifliddy/macbook12-audio-driver), via DKMS so
# kernel updates rebuild it
drv=/usr/src/macbook12-audio-driver-0.1
rm -rf "$drv" && install -d "$drv"
cp -a "$src"/macbook12-audio-driver/{dkms.conf,Makefile_cirrus,Makefile_cs420x,prepare.cirrus.driver.sh,patch_cirrus} "$drv"/
dkms add -m macbook12-audio-driver -v 0.1 || true
dkms install -m macbook12-audio-driver -v 0.1 -k "$kver" --force
modinfo -k "$kver" snd_hda_codec_cirrus | grep -q updates/

# Keyboard and trackpad (applespi, mainline since 5.3) in the initramfs, so they work from the
# first prompt on
for m in intel_lpss_pci spi_pxa2xx_platform spi_pxa2xx_pci applespi; do
    grep -qx "$m" /etc/initramfs-tools/modules || echo "$m" >> /etc/initramfs-tools/modules
done

# Apps: Nicotine+ (Soulseek) from Ubuntu, Google Chrome from Google's repo
apt_get install nicotine xinit x11-xserver-utils alsa-utils
install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /etc/apt/keyrings/google-chrome.gpg
echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/google-chrome.gpg] https://dl.google.com/linux/chrome/deb/ stable main" \
    > /etc/apt/sources.list.d/google-chrome.list
apt-get update
apt_get install google-chrome-stable

# Lighter: Chrome is the browser, so no snapd / Firefox snap; no office suite on a DJ machine
apt_get purge snapd firefox $(dpkg-query -W -f '${Package}\n' | grep '^libreoffice')
apt_get autoremove --purge

# Cratebox
install -m 0755 "$src/cratebox" /usr/bin/cratebox
install -Dm 0644 "$src/cratebox.desktop" /usr/share/applications/cratebox.desktop
install -Dm 0644 "$src/cratebox.png" /usr/share/icons/hicolor/512x512/apps/cratebox.png
gtk-update-icon-cache -q /usr/share/icons/hicolor || true
cp -a "$src"/overlay/. /

# Kiosk session. Live ISO: casper logs into lubuntu-live-environment.desktop, whose script we
# divert so Cratebox comes first. Installed system: Calamares sets SDDM to log straight into the
# Cratebox session.
dpkg-divert --local --rename --add --divert /usr/libexec/start-lubuntu-live-env.distrib /usr/libexec/start-lubuntu-live-env
install -m 0755 /usr/share/cratebox-kiosk/start-lubuntu-live-env /usr/libexec/start-lubuntu-live-env
sed -i -e 's/executable: "lxqt"/executable: "cratebox"/' -e 's/desktopFile: "Lubuntu"/desktopFile: "cratebox"/' \
    /etc/calamares/modules/displaymanager.conf
sed -i 's/^doAutologin: false/doAutologin: true/' /etc/calamares/modules/users.conf
grep -q 'desktopFile: "cratebox"' /etc/calamares/modules/displaymanager.conf
systemctl enable macbook12-audio-amps.service

update-initramfs -u -k "$kver"
apt-get clean
rm -rf /tmp/* /var/tmp/*
