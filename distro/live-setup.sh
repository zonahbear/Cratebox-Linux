#!/bin/bash
# Runs inside the live layer of the Lubuntu 26.04 ISO (installer, casper). The live session boots
# into Cratebox; the installer sets up an installed system that does the same.
set -euxo pipefail
src=/tmp/cratebox-os

# casper logs the live user into lubuntu-live-environment.desktop: its script now runs Cratebox
# first and the stock live session (Try / Install prompt) after
dpkg-divert --local --rename --add --divert /usr/libexec/start-lubuntu-live-env.distrib /usr/libexec/start-lubuntu-live-env
install -m 0755 "$src/start-lubuntu-live-env" /usr/libexec/start-lubuntu-live-env

# Calamares: log in automatically, into the Cratebox session
sed -i -e 's/executable: "lxqt"/executable: "cratebox"/' -e 's/desktopFile: "Lubuntu"/desktopFile: "cratebox"/' \
    /etc/calamares/modules/displaymanager.conf
sed -i 's/^doAutologin: false/doAutologin: true/' /etc/calamares/modules/users.conf
grep -q 'desktopFile: "cratebox"' /etc/calamares/modules/displaymanager.conf
grep -q '^doAutologin: true' /etc/calamares/modules/users.conf

# this layer's own copies of these caches predate the apps added below it
ldconfig
gtk-update-icon-cache -q -f /usr/share/icons/hicolor || true
