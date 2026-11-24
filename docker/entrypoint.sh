#!/usr/bin/env bash
# Boots the TC2 image under QEMU. Environment:
#   TC2_API        host:port of the test API (default host.docker.internal:1080),
#                  reachable in the guest as http://192.168.1.1:1080
#   TC2_GROUP      group to register the device in (default "new")
#   TC2_NAME       device name to register as (default: random)
#   TC2_AGENT=stand-in  run the tc2-emu-agent stand-in rather than the real
#                  tc2-agent against the emulated RP2040 (default: real)
#   TC2_RP2040=scripted  with the real tc2-agent, emulate the RP2040 with the
#                  scripted tc2-emu-rp2040 rather than tc2-firmware (tc2-firmware-emu)
#   TC2_PERSIST=1  keep guest disk changes between runs (default: fresh each run)
#   TC2_SMP        guest CPU count (default 4, as on a Pi Zero 2 W)
#   TC2_CONFIG_SYNC_DELAY  how long cacophony-config-sync waits after the config
#                  file changes before syncing, e.g. 100ms (default 10s, as on a
#                  real device)
#   TC2_AUTOLOGIN=0  no autologin on the serial console (default: log in as pi)
set -euo pipefail

IMAGES=${TC2_IMAGES:-/images}
STATE=${TC2_STATE:-/state}
EMU=${TC2_EMU:-/opt/tc2-emu}
RUN=$STATE/run.qcow2

for f in base.qcow2 vmlinuz initrd.img; do
    [[ -f $IMAGES/$f ]] || { echo "missing $IMAGES/$f - run 'make image'" >&2; exit 1; }
done

mkdir -p "$STATE"
if [[ ${TC2_PERSIST:-0} != 1 || ! -f $RUN ]]; then
    rm -f "$RUN"
    qemu-img create -q -f qcow2 -F qcow2 -b "$IMAGES/base.qcow2" "$RUN"
fi

# apparmor=0: Debian's kernel enables AppArmor by default, but the Raspberry Pi
# kernel a TC2 runs has CONFIG_LSM="" and cmdline.txt doesn't turn it on, so
# apparmor.service is skipped there. Here it took ~6s of the boot.
APPEND="root=PARTUUID=93147b3c-02 rootfstype=ext4 rootwait fsck.repair=yes console=ttyAMA0 net.ifnames=0 spidev.bufsiz=65536 apparmor=0"
[[ -n ${TC2_GROUP:-} ]] && APPEND+=" tc2-emu.group=$TC2_GROUP"
[[ -n ${TC2_NAME:-} ]] && APPEND+=" tc2-emu.name=$TC2_NAME"
case ${TC2_AGENT:-real} in
    real) APPEND+=" tc2-emu.agent=real" ;;
    stand-in) ;;
    *) echo "TC2_AGENT must be real or stand-in, not ${TC2_AGENT}" >&2; exit 1 ;;
esac
[[ -n ${TC2_CONFIG_SYNC_DELAY:-} ]] && APPEND+=" tc2-emu.config-sync-delay=$TC2_CONFIG_SYNC_DELAY"
[[ ${TC2_AUTOLOGIN:-} == 0 ]] && APPEND+=" tc2-emu.autologin=0"

# The guest configures eth0 statically as 192.168.1.2/24 with no gateway (the
# tc2-emu-eth0 service), so the user-mode network uses that subnet. QEMU presents forwarded connections with
# their real source address unless they come from loopback, and the guest can't
# route replies off-subnet, so published ports are relayed via 127.0.0.1.
# Guest connections to 192.168.1.1 arrive on the container's 127.0.0.1.
forward() {
    socat "TCP-LISTEN:$1,fork,reuseaddr" "TCP:$2" &
}
forward 2222 127.0.0.1:12222
forward 8080 127.0.0.1:18080
socat TCP-LISTEN:1080,bind=127.0.0.1,fork,reuseaddr "TCP:${TC2_API:-host.docker.internal:1080}" &

# Device models for the HAT. The guest's tc2-emu-* stubs reach them over a
# virtio-serial port, and load from $EMU/guest, shared into the guest over 9p.
[[ -x $EMU/host/tc2-emu-hw ]] || { echo "missing $EMU/host/tc2-emu-hw - run 'make stubs'" >&2; exit 1; }
"$EMU/host/tc2-emu-hw" --socket "$STATE/hw.sock" --rp2040-socket "$STATE/hw-rp2040.sock" \
    --listen 0.0.0.0:9000 &
# The RP2040 for the real tc2-agent: tc2-firmware on its mocked HAL, on the same I2C
# bus as the guest (tc2-emu-hw's RP2040 socket), its Lepton holding a fixture's frame.
if [[ ${TC2_AGENT:-real} == real ]]; then
    if [[ ${TC2_RP2040:-} == scripted ]]; then
        "$EMU/host/tc2-emu-rp2040" --socket "$STATE/rp2040.sock" &
    else
        [[ -x $EMU/host/tc2-firmware-emu ]] || { echo "missing $EMU/host/tc2-firmware-emu - run 'make stubs'" >&2; exit 1; }
        "$EMU/host/tc2-firmware-emu" --hw-socket "$STATE/hw-rp2040.sock" \
            --link "$STATE/rp2040.sock" --cptv "$EMU/guest/fixtures/cat-trigger.cptv" \
            --scenes "$EMU/guest/fixtures" --control 0.0.0.0:9100 &
    fi
fi

# 512M matches the Pi Zero 2 W, the lowest-spec hardware variant.
exec qemu-system-aarch64 \
    -M virt -cpu cortex-a53 -smp "${TC2_SMP:-4}" -m 512M \
    -kernel "$IMAGES/vmlinuz" -initrd "$IMAGES/initrd.img" -append "$APPEND" \
    -drive if=none,id=hd0,file="$RUN",format=qcow2,discard=unmap \
    -device virtio-blk-pci,drive=hd0 \
    -netdev user,id=net0,net=192.168.1.0/24,host=192.168.1.1,hostfwd=tcp:127.0.0.1:12222-192.168.1.2:22,hostfwd=tcp:127.0.0.1:18080-192.168.1.2:80 \
    -device virtio-net-pci,netdev=net0,romfile= \
    -device virtio-rng-pci \
    -device virtio-serial-pci \
    -chardev socket,id=hw,path="$STATE/hw.sock",server=on,wait=off \
    -device virtserialport,chardev=hw,name=org.cacophony.tc2-emu.hw \
    -chardev socket,id=rp2040,path="$STATE/rp2040.sock",server=on,wait=off \
    -device virtserialport,chardev=rp2040,name=org.cacophony.tc2-emu.rp2040 \
    -chardev file,id=journal,path="$STATE/journal.log" \
    -device virtserialport,chardev=journal,name=org.cacophony.tc2-emu.journal \
    -virtfs local,path="$EMU",mount_tag=tc2-emu,security_model=none,readonly=on \
    -display none -serial mon:stdio \
    -qmp unix:"$STATE/qmp.sock",server=on,wait=off \
    -no-shutdown
