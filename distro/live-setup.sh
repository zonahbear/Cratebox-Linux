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

# Calamares "Minimal installation": everything Cratebox OS adds lives in the full layer, so always
# unpack it, then let cratebox-minimal-install take out the Lubuntu apps a minimal install skips
cal=/etc/calamares
python3 - "$cal" <<'PY'
import re, sys
cal = sys.argv[1]
p = f"{cal}/modules/unpackfs.conf"
s = open(p).read()
s2 = s.replace('condition: "installation_data.unpack_regular"', 'condition: true')
assert s2 != s, "unpackfs.conf: standard-layer condition not found"
open(p, "w").write(s2)
p = f"{cal}/settings.conf"
s = open(p).read()
inst = "- id: cratebox_minimal\n  module: contextualprocess\n  config: cratebox_minimal_context.conf\n"
s2 = s.replace("sequence:\n", inst + "sequence:\n", 1)
s2 = re.sub(r"(\n(\s*)- contextualprocess@pkgselect_snap_action\n)", r"\1\2- contextualprocess@cratebox_minimal\n", s2, count=1)
assert s2.count("cratebox_minimal") == 3, "settings.conf: couldn't add the cratebox_minimal step"
open(p, "w").write(s2)
PY
cat > "$cal/modules/cratebox_minimal_context.conf" <<'CONF'
---
dontChroot: false
timeout: 3600
"installation_data.unpack_regular":
    false:
        - "/usr/libexec/cratebox-minimal-install"
CONF

# this layer's own copies of these caches predate the apps added below it
ldconfig
gtk-update-icon-cache -q -f /usr/share/icons/hicolor || true
