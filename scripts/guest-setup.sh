#!/usr/bin/env bash
# Runs inside the arm64 chroot of the TC2 rootfs, via prepare-image.sh, with
# this repo's guest/ directory at /tmp/tc2-emu-guest.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

# The QEMU host side of the guest's eth0 network. The test API is forwarded to
# port 1080 here (see docker/entrypoint.sh).
HOST_IP=192.168.1.1
API_URL=http://$HOST_IP:1080

# Emulator binaries (tc2-emu-*) are shared from the container over 9p.
EMU_DIR=/opt/tc2-emu

# Pi-hardware-only or unwanted in the emulator. Not masked, since some of these
# are unit files in /etc/systemd/system; tc2-emu.enable=<name> turns one back on.
DISABLED_SERVICES=(
    dphys-swapfile
    rpi-eeprom-update
    sshswitch
    salt-minion
    salt-updater
)

# Generic arm64 kernel for QEMU -M virt. The initramfs is built in the chroot, so
# it must not try to detect modules from the build host.
mkdir -p /etc/initramfs-tools/conf.d
echo 'MODULES=most' > /etc/initramfs-tools/conf.d/tc2-emulator
for mod in virtio_pci virtio_blk virtio_net; do
    grep -qx "$mod" /etc/initramfs-tools/modules || echo "$mod" >> /etc/initramfs-tools/modules
done
apt-get update
apt-get install -y --no-install-recommends linux-image-arm64
KVER=$(ls /lib/modules | grep -E -- '-arm64$' | sort -V | tail -n1)

# Build gpio-mockup for that kernel (see guest/gpio-mockup/README.md), removing
# the build dependencies again afterwards.
dpkg-query -W -f '${Package}\n' | sort > /tmp/packages-before
apt-get install -y --no-install-recommends "linux-headers-$KVER" gcc make libc6-dev
cp -r /tmp/tc2-emu-guest/gpio-mockup /tmp/gpio-mockup-build
make -C "/lib/modules/$KVER/build" M=/tmp/gpio-mockup-build modules
install -D -m 644 /tmp/gpio-mockup-build/gpio-mockup.ko "/lib/modules/$KVER/updates/gpio-mockup.ko"
depmod "$KVER"
dpkg-query -W -f '${Package}\n' | sort > /tmp/packages-after
comm -13 /tmp/packages-before /tmp/packages-after | xargs apt-get purge -y
rm -rf /tmp/gpio-mockup-build /tmp/packages-before /tmp/packages-after
apt-get clean

# Update the Cacophony software to the versions salt deploys, pinned in
# guest/versions/packages.txt, as saltops' cacophony.pkg_installed_from_github and
# pkg_installed_from_pypi states do: release debs installed with dpkg, and
# classifier-pipeline from PyPI into its venv.
while read -r pkg version; do
    case $pkg in
    '' | '#'*) continue ;;
    classifier-pipeline)
        /home/pi/.venv/classifier/bin/python3 -m pip install --no-cache-dir "$pkg==$version"
        continue
        ;;
    esac
    installed=$(dpkg-query -W -f '${Version}' "$pkg" 2>/dev/null | tr '~' '-' || true)
    [[ $installed == "$version" ]] && continue
    # Each package is released from the repo of the same name, but for go-config.
    repo=$pkg
    [[ $pkg == cacophony-config ]] && repo=go-config
    echo "Updating $pkg from ${installed:-none} to $version"
    curl -sfL -o "/tmp/$pkg.deb" \
        "https://github.com/TheCacophonyProject/$repo/releases/download/v$version/${pkg}_${version}_arm64.deb"
    # salt-updater's postinst runs salt-helper, which needs a salt minion id, so
    # give it one until device-register writes the real one on first boot.
    placeholder_minion_id=
    if [[ $pkg == salt-updater && ! -e /etc/salt/minion_id ]]; then
        echo pi-0 > /etc/salt/minion_id
        placeholder_minion_id=1
    fi
    dpkg -i "/tmp/$pkg.deb"
    [[ -n $placeholder_minion_id ]] && rm /etc/salt/minion_id
    rm "/tmp/$pkg.deb"
done < /tmp/tc2-emu-guest/versions/packages.txt

cat > /etc/modprobe.d/tc2-emu.conf <<'EOF'
# One chip at base 0 so sysfs GPIO numbers match the Pi's BCM numbers.
options gpio-mockup gpio_mockup_ranges=0,28
EOF
printf '%s\n' gpio-mockup 9p 9pnet_virtio > /etc/modules-load.d/tc2-emu.conf

# Input levels the HAT would normally hold, set before the hat services start:
# GPIO13 is pulled up in config.txt, and GPIO16 (ATtiny signal) idles high.
cat > /usr/local/sbin/tc2-emu-gpio-defaults <<'EOF'
#!/bin/sh
set -e
chip=$(ls -d /sys/kernel/debug/gpio-mockup/gpiochip* | head -n1)
for line in 13 16; do
    echo 1 > "$chip/$line"
done
EOF
chmod +x /usr/local/sbin/tc2-emu-gpio-defaults
cat > /etc/systemd/system/tc2-emu-gpio-defaults.service <<'EOF'
[Unit]
Description=Emulator: set default GPIO input levels
After=systemd-modules-load.service sys-kernel-debug.mount
Before=tc2-hat-attiny.service tc2-hat-i2c.service tc2-agent.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/tc2-emu-gpio-defaults

[Install]
WantedBy=multi-user.target
EOF
systemctl enable tc2-emu-gpio-defaults.service

mkdir -p "$EMU_DIR"
echo "tc2-emu $EMU_DIR 9p trans=virtio,version=9p2000.L,ro,nofail 0 0" >> /etc/fstab

# Run stubs from the share in place of services that need hardware: I2C goes to
# the emulated devices in tc2-emu-hw, and tc2-emu-agent fakes the RP2040 side of
# tc2-agent (or tc2-agent-launcher runs the real one against an emulated RP2040).
# tc2-hat-attiny and cacophony-config-sync run builds with guest/patches applied.
replace_service() {
    mkdir -p "/etc/systemd/system/$1.service.d"
    cat > "/etc/systemd/system/$1.service.d/tc2-emu.conf" <<EOF
[Unit]
RequiresMountsFor=$EMU_DIR

[Service]
ExecStart=
ExecStart=$EMU_DIR/guest/$2
EOF
}
replace_service tc2-hat-i2c tc2-hat-i2c-launcher
replace_service tc2-agent tc2-agent-launcher
replace_service tc2-hat-attiny "tc2-hat-controller attiny"
replace_service cacophony-config-sync "cacophony-config-utils sync"

for svc in "${DISABLED_SERVICES[@]}"; do
    mkdir -p "/etc/systemd/system/$svc.service.d"
    printf '[Unit]\nConditionKernelCommandLine=tc2-emu.enable=%s\n' "$svc" \
        > "/etc/systemd/system/$svc.service.d/tc2-emu.conf"
done
rm -f /var/swap

# The first-boot rc.local expands the rootfs on /dev/mmcblk0 then reboots. The
# emulated disk is already full size, so skip straight to the normal rc.local.
if [[ -f /etc/rc.local.bak ]]; then
    cp -f /etc/rc.local.bak /etc/rc.local
fi

# Fake internet. The guest has no default route, so it can't reach anything
# real, but services wait for connrequester (modemd) to see the internet: a
# ping to 8.8.8.8/8.8.4.4 and a DNS lookup of google.com. Answer both locally,
# resolving every name to the host side, where only the test API is reachable.
# tc2-emu-dns answers DNS rather than dnsmasq, which forwards AAAA queries to
# the image's unreachable upstream servers, so each lookup took 20s; dnsmasq is
# left with DHCP for the hotspot. prepare-image.sh keeps this resolv.conf.
cat > /usr/local/sbin/tc2-emu-uplink <<'EOF'
#!/bin/sh
set -e
ip link add tc2-emu0 type dummy 2>/dev/null || true
ip addr replace 8.8.8.8/32 dev tc2-emu0
ip addr replace 8.8.4.4/32 dev tc2-emu0
ip link set tc2-emu0 up
EOF
chmod +x /usr/local/sbin/tc2-emu-uplink
cat > /etc/systemd/system/tc2-emu-uplink.service <<'EOF'
[Unit]
Description=Emulator: answer internet connectivity checks locally
Before=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/tc2-emu-uplink

[Install]
WantedBy=multi-user.target
EOF
systemctl enable tc2-emu-uplink.service
cat > /etc/systemd/system/tc2-emu-dns.service <<EOF
[Unit]
Description=Emulator: fake DNS, resolving every name to $HOST_IP
RequiresMountsFor=$EMU_DIR
Before=nss-lookup.target network-online.target
Wants=nss-lookup.target

[Service]
ExecStart=$EMU_DIR/guest/tc2-emu-dns --address $HOST_IP
DynamicUser=yes
AmbientCapabilities=CAP_NET_BIND_SERVICE
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF
systemctl enable tc2-emu-dns.service
echo 'port=0' > /etc/dnsmasq.d/tc2-emu.conf
printf '[main]\ndns=none\n' > /etc/NetworkManager/conf.d/tc2-emu.conf
rm -f /etc/resolv.conf
echo 'nameserver 127.0.0.1' > /etc/resolv.conf

# Register with the test API rather than production. The group and name can be
# set with tc2-emu.group= and tc2-emu.name= on the kernel command line.
cat > /usr/local/bin/tc2-emu-device-register <<EOF
#!/bin/sh
group=new
name=
for arg in \$(cat /proc/cmdline); do
    case \$arg in
        tc2-emu.group=*) group=\${arg#*=} ;;
        tc2-emu.name=*) name=\${arg#*=} ;;
    esac
done
exec /usr/bin/device-register --reboot --retry-until-registered --api $API_URL \\
    --group "\$group" \${name:+--name "\$name"}
EOF
chmod +x /usr/local/bin/tc2-emu-device-register
mkdir -p /etc/systemd/system/device-register.service.d
cat > /etc/systemd/system/device-register.service.d/tc2-emu.conf <<'EOF'
[Service]
ExecStart=
ExecStart=/usr/local/bin/tc2-emu-device-register
EOF

# Stream this boot's journal to the container (/state/journal.log there) over a
# virtio-serial port, so it's readable without SSH and survives guest hangs and
# reboots. Starts as soon as the port appears and replays the boot so far. If it
# restarts within a boot, it carries on from then rather than replaying again:
# journalctl 252 doesn't save --cursor-file when stopped while following.
cat > /usr/local/sbin/tc2-emu-journal <<'EOF'
#!/bin/sh
started=/run/tc2-emu-journal.started
if [ -e $started ]; then
    echo "-- Journal stream restarted --"
    lines=0
else
    touch $started
    echo "-- Boot $(cat /proc/sys/kernel/random/boot_id) --"
    lines=all
fi
exec journalctl --boot --follow --lines=$lines --output=short-precise
EOF
chmod +x /usr/local/sbin/tc2-emu-journal
cat > /etc/systemd/system/tc2-emu-journal.service <<'EOF'
[Unit]
Description=Emulator: stream the journal to the host
DefaultDependencies=no
BindsTo=dev-virtio\x2dports-org.cacophony.tc2\x2demu.journal.device
After=dev-virtio\x2dports-org.cacophony.tc2\x2demu.journal.device systemd-journald.service

[Service]
ExecStart=/usr/local/sbin/tc2-emu-journal
StandardOutput=file:/dev/virtio-ports/org.cacophony.tc2-emu.journal
Restart=always
RestartSec=1

[Install]
WantedBy=dev-virtio\x2dports-org.cacophony.tc2\x2demu.journal.device
EOF
systemctl enable tc2-emu-journal.service

# Autologin on the serial console.
mkdir -p /etc/systemd/system/serial-getty@ttyAMA0.service.d
cat > /etc/systemd/system/serial-getty@ttyAMA0.service.d/autologin.conf <<'EOF'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin pi --keep-baud 115200,57600,38400,9600 %I $TERM
EOF

# SSH key for tests.
install -d -m 700 -o pi -g pi /home/pi/.ssh
cat /tmp/tc2-emu.pub >> /home/pi/.ssh/authorized_keys
chown pi:pi /home/pi/.ssh/authorized_keys
chmod 600 /home/pi/.ssh/authorized_keys

echo "built=$(date -Iseconds)" > /etc/tc2-emulator
