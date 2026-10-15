#!/bin/sh
# Install Cratebox for this user (no root needed): ~/.local/bin, the app menu entry and the icon.
#   ./install.sh            install / update
#   ./install.sh --remove   uninstall (your library data in ~/.local/share/cratebox is kept)
set -e
here="$(cd "$(dirname "$0")" && pwd)"
bin="$HOME/.local/bin"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"
if [ "$1" = "--remove" ]; then
  rm -f "$bin/cratebox" "$apps/cratebox.desktop" "$icons/cratebox.png"
  echo "Cratebox removed. Your library, playlists and cue points are still in ${XDG_DATA_HOME:-$HOME/.local/share}/cratebox."
  exit 0
fi
# built for x86-64-v3 (AVX2, FMA, BMI2): Intel Haswell (2013) / AMD Zen 1 or newer
if [ -r /proc/cpuinfo ] && ! grep -qw avx2 /proc/cpuinfo; then
  echo "This CPU has no AVX2, and this build of Cratebox needs it (x86-64-v3)."
  echo "Build it from source for this machine instead (see README)."
  exit 1
fi
mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/cratebox" "$bin/cratebox"
install -m 644 "$here/cratebox.png" "$icons/cratebox.png"
sed "s|^Exec=cratebox|Exec=$bin/cratebox|" "$here/cratebox.desktop" > "$apps/cratebox.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" >/dev/null 2>&1 || true
echo "Installed. Start it from your app menu (Cratebox) or run: $bin/cratebox"
case ":$PATH:" in *":$bin:"*) ;; *) echo "(Add $bin to your PATH to run 'cratebox' from a terminal.)";; esac
if ! [ -e /dev/snd/seq ]; then echo "Note: no ALSA sequencer (/dev/snd/seq), so controllers won't be seen: sudo modprobe snd-seq"; fi
