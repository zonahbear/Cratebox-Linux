#!/bin/bash
# Builds the Cratebox kiosk ISO by remastering the official Lubuntu 24.04 desktop ISO.
#   sudo distro/build-iso.sh LUBUNTU.iso CRATEBOX_BINARY OUT.iso
# Needs: squashfs-tools xorriso git curl, root (it chroots), ~25 GB free, network.
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: $0 LUBUNTU.iso CRATEBOX_BINARY OUT.iso" >&2; exit 2; }
iso=$(realpath "$1"); bin=$(realpath "$2"); out=$(realpath -m "$3")
here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
# pinned so a rebuild gets the driver that was tested
audio_driver_commit=4cdfcdbac2db3f300cc45d9679cfd21df6590a8a

work=${WORK:-$(dirname "$out")/kiosk-work}
rm -rf "$work" && mkdir -p "$work"
root="$work/rootfs"
cleanup() {
    for d in etc/ssl/certs/ca-certificates.crt run sys proc dev/pts dev; do umount -l "$root/$d" 2>/dev/null || true; done
}
trap cleanup EXIT

echo "== unpack"
xorriso -osirrox on -indev "$iso" -extract /casper/filesystem.squashfs "$work/fs.squashfs" -extract /boot/grub/grub.cfg "$work/grub.cfg"
unsquashfs -q -d "$root" "$work/fs.squashfs"
rm "$work/fs.squashfs"

echo "== stage files"
stage="$root/tmp/cratebox-kiosk"
mkdir -p "$stage"
cp -a "$here/overlay" "$stage/"
install -m 0755 "$bin" "$stage/cratebox"
cp "$repo/packaging/cratebox.desktop" "$repo/packaging/cratebox.png" "$stage/"
git clone -q https://github.com/leifliddy/macbook12-audio-driver "$stage/macbook12-audio-driver"
git -C "$stage/macbook12-audio-driver" checkout -q "$audio_driver_commit"
install -m 0755 "$here/chroot-setup.sh" "$stage/chroot-setup.sh"

echo "== chroot"
for d in dev dev/pts proc sys run; do mount --bind "/$d" "$root/$d"; done
# behind a TLS-intercepting proxy: BUILD_CA_BUNDLE=/path/ca.crt (only used during the build)
if [ -n "${BUILD_CA_BUNDLE:-}" ]; then
    cp "$BUILD_CA_BUNDLE" "$work/ca.crt"
    mount --bind "$work/ca.crt" "$root/etc/ssl/certs/ca-certificates.crt"
fi
mv "$root/etc/resolv.conf" "$work/resolv.conf.orig"
cp -L /etc/resolv.conf "$root/etc/resolv.conf"
chroot "$root" /tmp/cratebox-kiosk/chroot-setup.sh
rm "$root/etc/resolv.conf" && mv "$work/resolv.conf.orig" "$root/etc/resolv.conf"
cleanup

echo "== kernel, initrd, manifest, squashfs"
mkdir -p "$work/iso/casper" "$work/iso/boot/grub"
kver=$(ls "$root/lib/modules" | sort -V | tail -n1)
cp "$root/boot/vmlinuz-$kver" "$work/iso/casper/vmlinuz"
cp "$root/boot/initrd.img-$kver" "$work/iso/casper/initrd"
chroot "$root" dpkg-query -W --showformat='${Package}\t${Version}\n' > "$work/iso/casper/filesystem.manifest"
du -sx --block-size=1 "$root" | cut -f1 > "$work/iso/casper/filesystem.size"
mksquashfs "$root" "$work/iso/casper/filesystem.squashfs" -noappend -comp xz -b 1M -quiet
(cd "$work/iso/casper" && sha256sum filesystem.squashfs | sed 's/  / */' > SHA256SUMS)

# boot menu: Cratebox names, and the kiosk kernel options for the live session too
sed -e 's/Try or Install Lubuntu/Cratebox (try or install)/' -e 's/Lubuntu (safe graphics)/Cratebox (safe graphics)/' \
    -e 's|\(/casper/vmlinuz.*\) ---|\1 preempt=full ---|' "$work/grub.cfg" > "$work/iso/boot/grub/grub.cfg"

echo "== repack"
# md5sum.txt: the original's, with the changed files' sums replaced
xorriso -osirrox on -indev "$iso" -extract /md5sum.txt "$work/md5.orig"
for f in casper/vmlinuz casper/initrd casper/filesystem.squashfs casper/filesystem.manifest casper/filesystem.size casper/SHA256SUMS boot/grub/grub.cfg; do
    printf '%s  ./%s\n' "$(md5sum < "$work/iso/$f" | cut -d' ' -f1)" "$f"
done > "$work/md5.new"
{ awk 'NR == FNR { new[$2] = 1; next } !($2 in new)' "$work/md5.new" "$work/md5.orig"; cat "$work/md5.new"; } > "$work/iso/md5sum.txt"

rm -f "$out"
xorriso -indev "$iso" -outdev "$out" \
    -rm /casper/SHA256SUMS.gpg -- \
    -map "$work/iso/casper/vmlinuz" /casper/vmlinuz \
    -map "$work/iso/casper/initrd" /casper/initrd \
    -map "$work/iso/casper/filesystem.squashfs" /casper/filesystem.squashfs \
    -map "$work/iso/casper/filesystem.manifest" /casper/filesystem.manifest \
    -map "$work/iso/casper/filesystem.size" /casper/filesystem.size \
    -map "$work/iso/casper/SHA256SUMS" /casper/SHA256SUMS \
    -map "$work/iso/boot/grub/grub.cfg" /boot/grub/grub.cfg \
    -map "$work/iso/md5sum.txt" /md5sum.txt \
    -boot_image any replay
rm -rf "$work"
ls -l "$out"
