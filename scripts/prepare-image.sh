#!/usr/bin/env bash
# Converts a TC2 SD card image into what the emulator boots:
#   <outdir>/base.qcow2   compressed copy of the SD card, adapted for QEMU -M virt
#   <outdir>/vmlinuz      Debian generic arm64 kernel (the Pi kernel has no virtio)
#   <outdir>/initrd.img
#
# With a third argument, `device`, it makes an image to write to an SD card for a real
# TC2 instead, with the same software and changes but none of what's for QEMU (see
# guest-setup.sh), as <outdir>/tc2-real.img. That takes <outdir>/real_ed25519.pub, the
# wifi and test API in $REAL_CONF (real-device.conf), and the builds with guest/patches
# applied from build/guest (see the Makefile).
#
# Runs inside the builder container (see builder/Dockerfile). The source image is
# never modified.
set -euo pipefail

SRC=${1:?usage: prepare-image.sh <source.img> <outdir> [device]}
OUT=${2:?usage: prepare-image.sh <source.img> <outdir> [device]}
TARGET=${3:-emulator}
SCRIPTS=$(cd "$(dirname "$0")" && pwd)
WORK=$OUT/work.img
MNT=/mnt/tc2
ROOT_LOOP=
BOOT_LOOP=

log() { echo "==> $*" >&2; }

cleanup() {
    set +e
    for m in dev/pts dev proc sys boot/firmware; do
        mountpoint -q "$MNT/$m" && umount "$MNT/$m"
    done
    mountpoint -q "$MNT" && umount "$MNT"
    [[ -n $BOOT_LOOP ]] && losetup -d "$BOOT_LOOP"
    [[ -n $ROOT_LOOP ]] && losetup -d "$ROOT_LOOP"
}
trap cleanup EXIT

# Register qemu-aarch64 with the F flag so the interpreter is resolved now and
# works inside the chroot. This is host-global and lasts until reboot. Not on
# an arm64 host (e.g. Docker Desktop on Apple silicon), which runs the chroot
# natively, and where the handler would send every native binary through QEMU.
ensure_binfmt() {
    [[ $(uname -m) == aarch64 ]] && return
    local dir=/proc/sys/fs/binfmt_misc
    mountpoint -q "$dir" || mount -t binfmt_misc binfmt_misc "$dir"
    if [[ ! -e $dir/qemu-aarch64 ]]; then
        log "Registering qemu-aarch64 binfmt handler"
        echo ':qemu-aarch64:M::\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\xb7\x00:\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff:/usr/bin/qemu-aarch64-static:F' \
            > "$dir/register"
    fi
}

# Prints the loop device for partition $2 of image $1.
attach_partition() {
    local start sectors
    read -r start sectors < <(partx -g -o START,SECTORS -n "$2" "$1")
    losetup --find --show --offset $((start * 512)) --sizelimit $((sectors * 512)) "$1"
}

ensure_binfmt

log "Copying $SRC"
cp --sparse=always "$SRC" "$WORK"

ROOT_LOOP=$(attach_partition "$WORK" 2)
BOOT_LOOP=$(attach_partition "$WORK" 1)
e2fsck -fy "$ROOT_LOOP" || [[ $? -le 1 ]]

mkdir -p "$MNT"
mount "$ROOT_LOOP" "$MNT"
mount "$BOOT_LOOP" "$MNT/boot/firmware"
for m in dev dev/pts proc sys; do
    mount --bind "/$m" "$MNT/$m"
done

# Give the chroot working DNS; the guest's resolv.conf may be a symlink.
mv "$MNT/etc/resolv.conf" "$MNT/etc/resolv.conf.tc2-emu" 2>/dev/null || true
cp /etc/resolv.conf "$MNT/etc/resolv.conf"
# Don't let package scripts start services in the chroot.
printf '#!/bin/sh\nexit 101\n' > "$MNT/usr/sbin/policy-rc.d"
chmod +x "$MNT/usr/sbin/policy-rc.d"

cp "$SCRIPTS/guest-setup.sh" "$MNT/tmp/guest-setup.sh"
cp -r "$SCRIPTS/../guest" "$MNT/tmp/tc2-emu-guest"
if [[ $TARGET == device ]]; then
    cp "$OUT/real_ed25519.pub" "$MNT/tmp/tc2-emu.pub"
    cp "${REAL_CONF:?REAL_CONF must be the path of real-device.conf}" "$MNT/tmp/tc2-real.conf"
    mkdir -p "$MNT/tmp/tc2-real-bins"
    for bin in managementd tc2-agent tc2-hat-controller device-register cacophony-config-utils tc2-tzlookup event-reporter-tools; do
        cp "$SCRIPTS/../build/guest/$bin" "$MNT/tmp/tc2-real-bins/$bin"
    done
else
    cp "$OUT/id_ed25519.pub" "$MNT/tmp/tc2-emu.pub"
fi

log "Configuring guest ($TARGET)"
chroot "$MNT" env TC2_TARGET="$TARGET" /bin/bash /tmp/guest-setup.sh

rm -rf "$MNT/tmp/guest-setup.sh" "$MNT/tmp/tc2-emu.pub" "$MNT/tmp/tc2-emu-guest" "$MNT/usr/sbin/policy-rc.d" \
    "$MNT/tmp/tc2-real.conf" "$MNT/tmp/tc2-real-bins"
# Put the guest's resolv.conf back, unless guest-setup.sh replaced it.
if cmp -s /etc/resolv.conf "$MNT/etc/resolv.conf"; then
    rm -f "$MNT/etc/resolv.conf"
    mv "$MNT/etc/resolv.conf.tc2-emu" "$MNT/etc/resolv.conf" 2>/dev/null || true
else
    rm -f "$MNT/etc/resolv.conf.tc2-emu"
fi

if [[ $TARGET != device ]]; then
    log "Extracting kernel"
    KVER=$(ls "$MNT/lib/modules" | grep -E -- '-arm64$' | sort -V | tail -n1)
    cp "$MNT/boot/vmlinuz-$KVER" "$OUT/vmlinuz"
    cp "$MNT/boot/initrd.img-$KVER" "$OUT/initrd.img"
fi

(cleanup)
BOOT_LOOP=
ROOT_LOOP=$(attach_partition "$WORK" 2)
log "Zeroing free space"
e2fsck -fy "$ROOT_LOOP" || [[ $? -le 1 ]]
zerofree -v "$ROOT_LOOP"
losetup -d "$ROOT_LOOP"
ROOT_LOOP=

if [[ $TARGET == device ]]; then
    # A raw image for dd or Raspberry Pi Imager, which expands to the card on first boot.
    mv "$WORK" "$OUT/tc2-real.img"
    [[ -z ${HOST_UID:-} ]] || chown "$HOST_UID:${HOST_GID:-$HOST_UID}" "$OUT/tc2-real.img"
    log "Done: $(du -h --apparent-size "$OUT/tc2-real.img" | cut -f1) tc2-real.img (sparse, $(du -h "$OUT/tc2-real.img" | cut -f1) on disk)"
    exit 0
fi

log "Converting to compressed qcow2"
qemu-img convert -p -c -O qcow2 -o compression_type=zstd "$WORK" "$OUT/base.qcow2.tmp"
mv "$OUT/base.qcow2.tmp" "$OUT/base.qcow2"
rm -f "$WORK"

if [[ -n ${HOST_UID:-} ]]; then
    chown "$HOST_UID:${HOST_GID:-$HOST_UID}" "$OUT/base.qcow2" "$OUT/vmlinuz" "$OUT/initrd.img"
fi
log "Done (kernel $KVER): $(du -h "$OUT/base.qcow2" | cut -f1) base.qcow2"
