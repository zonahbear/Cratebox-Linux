#!/bin/bash
# Builds Cratebox OS by remastering the official Lubuntu 26.04 desktop ISO.
#   sudo distro/build-iso.sh LUBUNTU.iso CRATEBOX_BINARY OUT.iso
# Needs: squashfs-tools xorriso git curl, root (overlay mounts + chroot), ~25 GB free, network.
# SKIP_THIRD_PARTY=1 leaves out the out-of-tree speaker driver and Chrome (a quick check of the
# remaster itself; that image has no MacBook speakers).
#
# The 26.04 ISO is layered: casper/minimal.squashfs, then minimal.standard (the installed desktop),
# then minimal.standard.live (installer, casper). Each upper layer is an overlayfs upper dir over
# the ones below, so we unpack it, mount it as the upper dir again, and chroot into the result:
# every change lands in that layer, deletions as overlay whiteouts. The minimal layer, the kernel
# and the live initrd stay exactly as shipped.
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: $0 LUBUNTU.iso CRATEBOX_BINARY OUT.iso" >&2; exit 2; }
iso=$(realpath "$1"); bin=$(realpath "$2"); out=$(realpath -m "$3")
here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
# the commit whose patch_cirrus files match the ones verified on Jonah's MacBook10,1
audio_driver_commit=4cdfcdbac2db3f300cc45d9679cfd21df6590a8a

work=${WORK:-$(dirname "$out")/cratebox-os-work}
rm -rf "$work" && mkdir -p "$work"/{iso/casper,iso/boot/grub,ovl}
mounts=()
umount_all() {
    local i
    for ((i = ${#mounts[@]} - 1; i >= 0; i--)); do umount -l "${mounts[i]}" 2>/dev/null || true; done
    mounts=()
}
trap umount_all EXIT

# chroot_layer UPPER LOWERS SCRIPT: overlay UPPER on LOWERS (colon list, top first), run SCRIPT inside
chroot_layer() {
    local upper=$1 lowers=$2 script=$3 name
    name=$(basename "$upper")
    local root="$work/ovl/$name-root" wd="$work/ovl/$name-work"
    mkdir -p "$root" "$wd"
    mount -t overlay overlay -o "lowerdir=$lowers,upperdir=$upper,workdir=$wd,redirect_dir=off,index=off,metacopy=off,xino=off" "$root"; mounts+=("$root")
    for d in dev dev/pts proc sys run; do mount --bind "/$d" "$root/$d"; mounts+=("$root/$d"); done
    if [ -n "${BUILD_CA_BUNDLE:-}" ]; then  # behind a TLS-intercepting proxy, during the build only
        cp "$BUILD_CA_BUNDLE" "$work/ca.crt"
        mount --bind "$work/ca.crt" "$root/etc/ssl/certs/ca-certificates.crt"; mounts+=("$root/etc/ssl/certs/ca-certificates.crt")
    fi
    mv "$root/etc/resolv.conf" "$work/resolv.conf.orig"
    cp -L /etc/resolv.conf "$root/etc/resolv.conf"
    chroot "$root" env SKIP_THIRD_PARTY="${SKIP_THIRD_PARTY:-}" "$script"
    rm "$root/etc/resolv.conf" && mv "$work/resolv.conf.orig" "$root/etc/resolv.conf"
    chroot "$root" dpkg-query -W --showformat='${Package}\t${Version}\n' > "$work/$name.manifest.full"
    rm -rf "$root/tmp/cratebox-os"
    umount_all
    rm -rf "$wd"
}

echo "== unpack"
for l in minimal minimal.standard minimal.standard.live; do
    xorriso -osirrox on -indev "$iso" -extract "/casper/$l.squashfs" "$work/$l.squashfs" -extract "/casper/$l.manifest.full" "$work/$l.manifest.full.orig"
    unsquashfs -q -no-progress -d "$work/$l" "$work/$l.squashfs"
    rm "$work/$l.squashfs"
done
xorriso -osirrox on -indev "$iso" -extract /boot/grub/grub.cfg "$work/grub.cfg" -extract /md5sum.txt "$work/md5.orig"
M="$work/minimal" S="$work/minimal.standard" L="$work/minimal.standard.live"

echo "== installed system (minimal.standard layer)"
stage="$S/tmp/cratebox-os"
mkdir -p "$stage"
cp -a "$here/overlay" "$stage/"
install -m 0755 "$bin" "$stage/cratebox"
cp "$repo/packaging/cratebox.desktop" "$repo/packaging/cratebox.png" "$stage/"
if [ -z "${SKIP_THIRD_PARTY:-}" ]; then
    git clone -q https://github.com/leifliddy/macbook12-audio-driver "$stage/macbook12-audio-driver"
    git -C "$stage/macbook12-audio-driver" checkout -q "$audio_driver_commit"
fi
install -m 0755 "$here/chroot-setup.sh" "$stage/"
cp "$S/var/lib/dpkg/status" "$work/standard.status.orig"
chroot_layer "$S" "$M" /tmp/cratebox-os/chroot-setup.sh
"$here/merge-dpkg-status.py" "$work/standard.status.orig" "$S/var/lib/dpkg/status" "$L/var/lib/dpkg/status" "$work/live.status"
cp "$work/live.status" "$L/var/lib/dpkg/status"

echo "== live session (minimal.standard.live layer)"
mkdir -p "$L/tmp/cratebox-os"
install -m 0755 "$here/live-setup.sh" "$here/live/start-lubuntu-live-env" "$L/tmp/cratebox-os/"
chroot_layer "$L" "$S:$M" /tmp/cratebox-os/live-setup.sh

echo "== squash"
for l in minimal.standard minimal.standard.live; do
    mksquashfs "$work/$l" "$work/iso/casper/$l.squashfs" -noappend -comp xz -quiet
    du -sx --block-size=1 "$work/$l" | cut -f1 > "$work/iso/casper/$l.size"
    cp "$work/$l.manifest.full" "$work/iso/casper/$l.manifest.full"
done
# .manifest = what the layer adds or removes against the one below
diff -u --label minimal.manifest.full "$work/minimal.manifest.full.orig" \
    --label minimal.standard.manifest.full "$work/minimal.standard.manifest.full" > "$work/iso/casper/minimal.standard.manifest" || true
diff -u --label minimal.standard.manifest.full "$work/minimal.standard.manifest.full" \
    --label minimal.standard.live.manifest.full "$work/minimal.standard.live.manifest.full" > "$work/iso/casper/minimal.standard.live.manifest" || true

# boot menu: Cratebox names, and the same kernel options as the installed system
sed -e 's/Try or Install Lubuntu/Cratebox OS (try or install)/' -e 's/Lubuntu (safe graphics)/Cratebox OS (safe graphics)/' \
    -e 's|\(/casper/vmlinuz.*\) ---|\1 mem_sleep_default=s2idle preempt=full ---|' "$work/grub.cfg" > "$work/iso/boot/grub/grub.cfg"

echo "== repack"
changed=(boot/grub/grub.cfg)
for l in minimal.standard minimal.standard.live; do
    changed+=("casper/$l.squashfs" "casper/$l.size" "casper/$l.manifest" "casper/$l.manifest.full")
done
for f in "${changed[@]}"; do
    printf '%s  ./%s\n' "$(md5sum < "$work/iso/$f" | cut -d' ' -f1)" "$f"
done > "$work/md5.new"
{ awk 'NR == FNR { new[$2] = 1; next } !($2 in new)' "$work/md5.new" "$work/md5.orig"; cat "$work/md5.new"; } > "$work/iso/md5sum.txt"
maps=()
for f in "${changed[@]}" md5sum.txt; do maps+=(-map "$work/iso/$f" "/$f"); done
rm -f "$out"
xorriso -indev "$iso" -outdev "$out" "${maps[@]}" -boot_image any replay
rm -rf "$work"
ls -l "$out"
