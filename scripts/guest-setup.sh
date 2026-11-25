#!/usr/bin/env bash
# Runs inside the arm64 chroot of the TC2 rootfs, via prepare-image.sh, with
# this repo's guest/ directory at /tmp/tc2-emu-guest.
#
# TC2_TARGET=emulator (the default) makes the image the emulator boots. TC2_TARGET=device
# makes one to write to an SD card for a real TC2, to compare with the emulator: it has
# the same software and the same changes, but none of what's for QEMU, and the builds
# with guest/patches applied are installed over the packages' binaries instead of being
# run from the emulator's share. Everything that's emulator-only is in `if emulator`.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

TARGET=${TC2_TARGET:-emulator}
emulator() { [[ $TARGET == emulator ]]; }
if ! emulator; then
    # The wifi network and test API for the device, from real-device.conf.
    # shellcheck disable=SC1091
    [[ ! -f /tmp/tc2-real.conf ]] || . /tmp/tc2-real.conf
    # Fail now, not after the build, rather than register a device with production.
    if [[ -n ${API_URL:-} ]]; then
        if [[ $API_URL != http://* && $API_URL != https://* ]]; then
            echo "ERROR: API_URL in real-device.conf isn't an http(s) URL: $API_URL" >&2
            exit 1
        fi
        if [[ $API_URL == *cacophony.org.nz* ]]; then
            echo "ERROR: API_URL in real-device.conf is a production API: $API_URL" >&2
            exit 1
        fi
    fi
fi

# The QEMU host side of the guest's eth0 network. The test API is forwarded to
# port 1080 here (see docker/entrypoint.sh).
HOST_IP=192.168.1.1
EMU_API_URL=http://$HOST_IP:1080

# Emulator binaries (tc2-emu-*) are shared from the container over 9p.
EMU_DIR=/opt/tc2-emu

# Not run in the emulator or on the device. Not masked, since some of these are unit
# files in /etc/systemd/system; tc2-emu.enable=<name> on the kernel command line (in
# cmdline.txt, on a device) turns one back on.
DISABLED_SERVICES=(
    # No salt master to take orders from. On a device this would update it behind our
    # back as well as contacting the production salt master.
    salt-minion
    salt-updater
    # Loads the console keymap for a keyboard on the HDMI console, which a TC2
    # never has (and the layout is "us", the kernel's default anyway): the cached
    # /etc/console-setup/cached_setup_keyboard.sh (kbd_mode on tty1-6, loadkeys),
    # ~1.4s on an idle guest and ~2.6s at boot.
    # TODO: disable keyboard-setup in the real TC2 image too, then this entry
    # just matches it.
    keyboard-setup
    # Global hotkey daemon reading /dev/input/event*; a TC2 has no input devices.
    # TODO: disable triggerhappy (service and socket) in the real TC2 image too,
    # then this entry just matches it.
    triggerhappy
    # Desktop removable-media automounter; no one plugs storage into a TC2.
    # TODO: disable udisks2 in the real TC2 image too, then this entry just
    # matches it.
    udisks2
    # Login prompt on the HDMI console, which never has a monitor or keyboard.
    # TODO: disable getty@tty1 in the real TC2 image too, then this entry just
    # matches it.
    getty@tty1
    # Turns off the official Pi DSI display's backlight at shutdown; no display.
    # It's already skipped here (ConditionPathIsDirectory=/proc/device-tree/
    # rpi_backlight), so this entry changes nothing in the emulator.
    # TODO: disable rpi-display-backlight in the real TC2 image too, then this
    # entry just matches it.
    rpi-display-backlight
)
# What a real TC2 has that the emulator doesn't.
EMULATOR_DISABLED_SERVICES=(
    rpi-eeprom-update
    sshswitch
    # No USB modem. It only starts once registering makes /etc/salt/minion_id,
    # then exits at once and restarts every 5s, loading the guest.
    modemd
    # No wlan0. It tries to start the hotspot anyway, taking NetworkManager down
    # (and with it the host's connections to managementd), then exits and restarts.
    rpi-net-manager
)
if emulator; then
    DISABLED_SERVICES+=("${EMULATOR_DISABLED_SERVICES[@]}")
fi

if emulator; then
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
fi

# Update the Cacophony software to the versions salt deploys, pinned in
# guest/versions/packages.txt, as saltops' cacophony.pkg_installed_from_github and
# pkg_installed_from_pypi states do: release debs installed with dpkg, and
# classifier-pipeline from PyPI into its venv.
while read -r pkg version; do
    case $pkg in
    '' | '#'*) continue ;;
    classifier-pipeline)
        /home/pi/.venv/classifier/bin/python3 -m pip install --no-cache-dir "$pkg==$version"
        # guest/patches/classifier-pipeline, from the repo's src/ to the installed package.
        for patch in /tmp/tc2-emu-guest/patches/classifier-pipeline/*.patch; do
            patch -p2 -d /home/pi/.venv/classifier/lib/python3.11/site-packages < "$patch"
        done
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

# The rest of what salt deploys that the older image doesn't have yet: a highstate
# of the pinned saltops commit (guest/versions/saltops.txt), see
# guest/salt/apply-salt.sh. The Cacophony packages are already the pinned versions
# from above, so this is files, models and service enablement.
/tmp/tc2-emu-guest/salt/apply-salt.sh

# thermal-recorder-py starts as soon as it can: after the root filesystem is writable, not
# after local-fs.target (active ~8.8s into boot on a Pi Zero 2 W, waiting on the other
# mounts) and systemd-modules-load, which it doesn't need. Its Python takes ~7.7s to start
# and the stream from the RP2040 can't be handed to it before then, so in medium power mode
# the first classification waits on this. It measured 3.6s earlier listening on a device.
# This is upstream-patches/saltops/0004, which salt would otherwise put back. (A drop-in can't
# do it: an `After=` reset leaves local-fs.target in the unit's list.) It needs the retry
# for the system bus in guest/patches/classifier-pipeline/0005, as it can now start before
# D-Bus does.
recorder_unit=/etc/systemd/system/thermal-recorder-py.service
grep -qx 'After=local-fs.target systemd-modules-load.service' "$recorder_unit" ||
    { echo "$recorder_unit has changed, update the After= change in guest-setup.sh" >&2; exit 1; }
sed -i 's/^After=local-fs.target systemd-modules-load.service$/After=systemd-remount-fs.service/' "$recorder_unit"

# classifier-pipeline downloads label_paths.json from the main branch of
# cacophony-web the first time it loads a model, into its working directory, which
# for the thermal-recorder-py service is /. A device does that itself when it has
# internet, but the emulated one has none, so without the file the models never
# load and nothing is classified. This is a device that has already downloaded it.
if emulator; then
    curl -sfL -o /label_paths.json \
        https://raw.githubusercontent.com/TheCacophonyProject/cacophony-web/main/api/classifications/label_paths.json
fi

# No salt master. The image's minion config retries it forever (master_tries:
# -1), so salt-call without --local never exits: managementd runs `salt-call
# grains.setvals` (go-utils saltutil.SetGrains) on each reregister, and these
# piled up until the guest ran out of memory. Run salt-call masterless instead.
mkdir -p /etc/salt/minion.d
echo 'file_client: local' > /etc/salt/minion.d/tc2-emu.conf
if emulator; then
# Even masterless, each salt-call takes ~38MB and minutes to start on the guest,
# and with the e2e tests reregistering every few seconds they still piled up
# until it ran out of memory. Use the stub on the share (guest/bin/salt-call),
# which sets grains directly; the real one stays as salt-call.distrib.
dpkg-divert --local --rename --add /usr/bin/salt-call
ln -sf "$EMU_DIR/guest/salt-call" /usr/bin/salt-call

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
# tc2-hat-attiny, cacophony-config-sync and managementd run builds with
# guest/patches applied, managementd's against the patched go-config.
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
replace_service cacophony-config-sync cacophony-config-sync-launcher
replace_service managementd managementd
# Not before the device registers, as managementd isn't: otherwise they start, fail
# for want of a registration and wait to restart. device-register starts them once
# /etc/salt/minion_id exists. This is what upstream-patches/tc2-agent/0001 and
# upstream-patches/go-config/0002 do to the units themselves.
for unit in tc2-agent cacophony-config-sync; do
    printf '\n[Unit]\nConditionPathExists=/etc/salt/minion_id\n' >> "/etc/systemd/system/$unit.service.d/tc2-emu.conf"
done
# thermal-recorder-py exits whenever /etc/cacophony/config.toml changes, even in sections
# it doesn't read, and systemd restarts it after RestartSec=3s. Every e2e test that sets
# config pays that wait before a recording can be taken, so don't wait here. (Upstream
# the exit is the simple way to reload its config, which several processes copy at
# start; the restart is wasteful, as is its slow Python start, but not ours to change here.)
# What upstream-patches/tc2-hat-controller/0002 does to the units: tc2-hat-i2c counts as
# started once it owns org.cacophony.i2c, and tc2-hat-rtc and tc2-hat-attiny order after it,
# as both need it at start, and so does tc2-agent (it reads and writes the ATtiny over
# it, not through tc2-hat-attiny; upstream-patches/tc2-agent/0002). Without that, they raced it at boot: tc2-hat-rtc failed its
# first check and retried every 3s, and tc2-hat-attiny read "no I2C service" as "no ATtiny".
tc2_ordering_dropin() {
    mkdir -p "/etc/systemd/system/$1.service.d"
    cat > "/etc/systemd/system/$1.service.d/tc2-ordering.conf"
}
tc2_ordering_dropin tc2-hat-i2c <<'EOF'
[Service]
Type=dbus
BusName=org.cacophony.i2c
EOF
for unit in tc2-hat-rtc tc2-hat-attiny tc2-agent; do
    tc2_ordering_dropin "$unit" <<'EOF'
[Unit]
After=tc2-hat-i2c.service
Wants=tc2-hat-i2c.service
EOF
done
# The tc2-agent package's postinst asks it to offload and erase the RP2040 flash on its
# first run, after which it exits and waits RestartSec=5s to restart. The emulated flash
# starts empty and is never erased, so on every fresh boot that is a pointless exit.
if emulator; then
    rm -f /etc/cacophony/offload_erase
fi
mkdir -p /etc/systemd/system/thermal-recorder-py.service.d
printf '[Service]\nRestartSec=0\n' > /etc/systemd/system/thermal-recorder-py.service.d/tc2-emu.conf
else
# The builds with guest/patches applied (see the Makefile's real-image target) go over
# the packages' binaries. The units run them as they do the packages'; the emulator's
# patches add flags (--rp2040-link, --sync-delay) that are off unless given.
for bin in managementd tc2-agent tc2-rp2040-logs tc2-hat-controller device-register cacophony-config-utils tc2-tzlookup event-reporter-tools; do
    install -m 755 "/tmp/tc2-real-bins/$bin" "/usr/bin/$bin"
done
# What the unit changes in upstream-patches do, until the packages' units have them (the
# emulator does the same with the drop-ins above). The packages' own units stay as they are.
# - tc2-agent/0001 and go-config/0002: not before the device registers (as managementd and
#   tc2-hat-attiny aren't); device-register starts them once /etc/salt/minion_id exists.
# - tc2-agent/0002 and tc2-hat-controller/0002: tc2-hat-i2c counts as started once it owns
#   org.cacophony.i2c, and tc2-hat-rtc, tc2-hat-attiny and tc2-agent order after it.
#   Measured on a real device (4 boots each): camera ready 1.0s sooner.
for unit in tc2-agent cacophony-config-sync; do
    mkdir -p "/etc/systemd/system/$unit.service.d"
    printf '[Unit]\nConditionPathExists=/etc/salt/minion_id\n' > "/etc/systemd/system/$unit.service.d/tc2-upstream.conf"
done
mkdir -p /etc/systemd/system/tc2-hat-i2c.service.d
printf '[Service]\nType=dbus\nBusName=org.cacophony.i2c\n' > /etc/systemd/system/tc2-hat-i2c.service.d/tc2-upstream.conf
for unit in tc2-hat-rtc tc2-hat-attiny tc2-agent; do
    mkdir -p "/etc/systemd/system/$unit.service.d"
    printf '[Unit]\nAfter=tc2-hat-i2c.service\nWants=tc2-hat-i2c.service\n' >> "/etc/systemd/system/$unit.service.d/tc2-upstream.conf"
done
fi

# managementd runs socket activated (guest/patches/management-interface 0003):
# systemd listens on port 80 and starts it for the first connection, and it exits
# when idle, so it and tc2-agent's frame forwarding to it don't run with nobody
# using the web interface. The unit is the one the patch adds to the managementd
# package; the image's package doesn't have it yet, and still enables the service.
cat > /etc/systemd/system/managementd.socket <<'EOF'
[Unit]
Description=Cacophonator management interface socket
ConditionPathExists=/etc/salt/minion_id

[Socket]
ListenStream=80

[Install]
WantedBy=multi-user.target
EOF
systemctl disable managementd.service
systemctl enable managementd.socket

if emulator; then
    rm -f /var/swap
fi

if emulator; then
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

# Swap is on, as on a real TC2 (dphys-swapfile, with the swap file size and swappiness salt
# sets), so memory use that would swap there does here. While we audit that nothing swaps
# on an SD card, report it as an event. Emulator only; see guest/bin/tc2-emu-swap-watcher.
cat > /etc/systemd/system/tc2-emu-swap-watcher.service <<EOF
[Unit]
Description=Emulator: report swap use as an event
After=event-reporter.service
RequiresMountsFor=$EMU_DIR

[Service]
ExecStart=$EMU_DIR/guest/tc2-emu-swap-watcher
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
systemctl enable tc2-emu-swap-watcher.service
# tc2-hat-rtc sets the clock from the RTC's whole seconds, so the guest is up to a
# second behind the host; set it exactly. See guest/bin/tc2-emu-timesync.
cat > /etc/systemd/system/tc2-emu-timesync.service <<EOF
[Unit]
Description=Emulator: set the clock from the host to the millisecond
After=tc2-hat-rtc.service tc2-emu-eth0.service
RequiresMountsFor=$EMU_DIR

[Service]
# Not oneshot, which would hold up multi-user.target (and device-register after it)
# while this waits for tc2-hat-rtc.
Type=simple
ExecStart=$EMU_DIR/guest/tc2-emu-timesync

[Install]
WantedBy=multi-user.target
EOF
systemctl enable tc2-emu-timesync.service
echo 'port=0' > /etc/dnsmasq.d/tc2-emu.conf
fi

# The guest's eth0 is the link to the host (QEMU user networking). The image
# configures it statically through ifupdown (/etc/network/interfaces.d/eth0,
# from saltops tc2/network-manager, only for debugging with a laptop plugged in,
# which nobody does any more). With an interface defined there, ifupdown-pre
# runs `udevadm settle` and networking.service runs ifup with its hooks, together
# ~12s of the emulator's boot. Remove it and set the address here instead.
# TODO: remove /etc/network/interfaces.d/eth0 (the saltops state) from the real
# TC2 image too, then only the tc2-emu-eth0 service below is emulator-specific.
rm -f /etc/network/interfaces.d/eth0
# Nothing else on a TC2 uses ifupdown (NetworkManager does the wifi, driven by
# rpi-net-manager, and the USB modem's usb0; none of the Cacophony software calls
# ifup) or NFS, yet they cost ~7s of CPU at boot, which is all there is to do on the
# emulated CPU: networking.service's hook scripts (~1s idle, 4.8s at boot),
# 80-ifupdown.rules running `ifup --allow=hotplug` for each network device, and
# 99-nfs.rules running `nfsrahead` (which always fails) for each block device.
# TODO: purge ifupdown and nfs-common from the real TC2 image too (a pkg.purged
# in saltops' tc2/basics, with the eth0 state removed), then this just matches it.
apt-get purge -y ifupdown nfs-common
# console-setup's udev rules load the console font and set UTF-8 mode on each of the
# virtual consoles as they appear, ~1s of CPU at boot, for the HDMI console only a
# monitor plugged into a TC2 would show (the kernel's font and UTF-8 default do).
# This also removes console-setup.service. keyboard-setup stays, disabled above.
# TODO: purge console-setup and console-setup-linux from the real TC2 image too
# (add them to the pkg.purged in saltops' tc2/basics), then this just matches it.
apt-get purge -y console-setup console-setup-linux
# Nobody reads a manpage on a TC2, and man-db has a dpkg trigger that rebuilds its
# index whenever a package that installs one is installed, as salt's updates do.
# TODO: purge man-db, manpages, manpages-dev and groff-base from the real TC2 image
# too (the pkg.purged in saltops' tc2/basics), then this just matches it.
apt-get purge -y man-db manpages manpages-dev groff-base

# logrotate.timer is Persistent, so it runs at boot to catch up on a missed day, with
# the rest of the system starting (~5s here, for what are a few KB of apt, dpkg and
# wtmp logs; journald looks after the rest). Run it once the system has been up a
# while instead, and then daily.
# TODO: the same in the real TC2 image (a drop-in from saltops' tc2/basics).
mkdir -p /etc/systemd/system/logrotate.timer.d
cat > /etc/systemd/system/logrotate.timer.d/tc2.conf <<'EOF'
[Timer]
OnCalendar=
Persistent=false
AccuracySec=1min
OnBootSec=5min
OnUnitActiveSec=1d
EOF
if emulator; then
cat > /etc/systemd/system/tc2-emu-eth0.service <<EOF
[Unit]
Description=Emulator: address the host link on eth0
Requires=sys-subsystem-net-devices-eth0.device
After=sys-subsystem-net-devices-eth0.device
Before=network-pre.target
Wants=network-pre.target
DefaultDependencies=no

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/sbin/ip addr replace 192.168.1.2/24 dev eth0
ExecStart=/sbin/ip link set eth0 up

[Install]
WantedBy=sysinit.target
EOF
systemctl enable tc2-emu-eth0.service
# Without the ifupdown entry NetworkManager would manage eth0 and run DHCP on it.
printf '[main]\ndns=none\n\n[keyfile]\nunmanaged-devices=interface-name:eth0\n' > /etc/NetworkManager/conf.d/tc2-emu.conf
rm -f /etc/resolv.conf
echo 'nameserver 127.0.0.1' > /etc/resolv.conf
fi

if emulator; then
# Register with the test API rather than production. The group and name can be
# set with tc2-emu.group= and tc2-emu.name= on the kernel command line. Runs the
# patched device-register from the share, which starts the services that need a
# registered device rather than rebooting, as the patched release's unit does.
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
exec $EMU_DIR/guest/device-register --start-services --retry-until-registered --api $EMU_API_URL \\
    --group "\$group" \${name:+--name "\$name"}
EOF
chmod +x /usr/local/bin/tc2-emu-device-register
mkdir -p /etc/systemd/system/device-register.service.d
cat > /etc/systemd/system/device-register.service.d/tc2-emu.conf <<EOF
[Unit]
RequiresMountsFor=$EMU_DIR

[Service]
ExecStart=
ExecStart=/usr/local/bin/tc2-emu-device-register
EOF
else
# Register with the test API from real-device.conf, never production, with the
# patched device-register, which starts the services that need a registered device
# rather than rebooting, as the patched release's unit does. Without a test API the
# device stays unregistered and device-register is disabled, so that nothing contacts
# the production API; everything that needs a registered device then doesn't run.
if [[ -n ${API_URL:-} ]]; then
    mkdir -p /etc/systemd/system/device-register.service.d
    # Sorts after the overwrite.conf salt installs there.
    cat > /etc/systemd/system/device-register.service.d/tc2-real.conf <<EOF
[Service]
ExecStart=
ExecStart=/usr/bin/device-register --start-services --retry-until-registered --prefix=tc2 --api $API_URL --group ${GROUP:-new}
EOF
else
    DISABLED_SERVICES+=(device-register)
fi
fi

if emulator; then
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

# Autologin on the serial console, unless tc2-emu.autologin=0 (TC2_AUTOLOGIN=0,
# as `make e2e` sets): the login session it starts costs a few seconds of CPU
# just as the device registers.
cat > /usr/local/sbin/tc2-emu-serial-getty <<'EOF'
#!/bin/sh
# Usage: tc2-emu-serial-getty <tty> <term>
autologin="--autologin pi"
grep -qw tc2-emu.autologin=0 /proc/cmdline && autologin=
exec /sbin/agetty $autologin --keep-baud 115200,57600,38400,9600 "$1" "$2"
EOF
chmod +x /usr/local/sbin/tc2-emu-serial-getty
mkdir -p /etc/systemd/system/serial-getty@ttyAMA0.service.d
cat > /etc/systemd/system/serial-getty@ttyAMA0.service.d/autologin.conf <<'EOF'
[Service]
ExecStart=
ExecStart=-/usr/local/sbin/tc2-emu-serial-getty %I $TERM
EOF
fi

# After everything above has had its say on DISABLED_SERVICES.
for svc in "${DISABLED_SERVICES[@]}"; do
    mkdir -p "/etc/systemd/system/$svc.service.d"
    printf '[Unit]\nConditionKernelCommandLine=tc2-emu.enable=%s\n' "$svc" \
        > "/etc/systemd/system/$svc.service.d/tc2-emu.conf"
done

if ! emulator; then
    # The wifi network to join, from real-device.conf, which takes priority over the
    # bushnet network the image already knows. There's no ethernet on a TC2, so this is
    # the only way to reach it. NetworkManager keyfiles must be root-only.
    if [[ -n ${WIFI_SSID:-} ]]; then
        {
            echo '[connection]'
            echo "id=$WIFI_SSID"
            echo "uuid=$(cat /proc/sys/kernel/random/uuid)"
            echo 'type=wifi'
            echo 'autoconnect-priority=100'
            echo 'interface-name=wlan0'
            echo
            echo '[wifi]'
            echo 'mode=infrastructure'
            echo "ssid=$WIFI_SSID"
            [[ ${WIFI_HIDDEN:-} != yes ]] || echo 'hidden=true'
            if [[ -n ${WIFI_PSK:-} ]]; then
                echo
                echo '[wifi-security]'
                echo 'auth-alg=open'
                echo 'key-mgmt=wpa-psk'
                echo "psk=$WIFI_PSK"
            fi
            echo
            echo '[ipv4]'
            echo 'method=auto'
            echo 'route-metric=10'
            echo
            echo '[ipv6]'
            echo 'addr-gen-mode=default'
            echo 'method=auto'
            echo 'route-metric=10'
        } > /etc/NetworkManager/system-connections/tc2-real-wifi.nmconnection
        chmod 600 /etc/NetworkManager/system-connections/tc2-real-wifi.nmconnection
    else
        echo "WARNING: no WIFI_SSID in real-device.conf: the device will only have the hotspot" >&2
    fi
    # The wifi regulatory domain, if the image doesn't have one set and the network needs it.
    if [[ -n ${WIFI_COUNTRY:-} ]] && ! grep -q 'cfg80211.ieee80211_regdom' /boot/firmware/cmdline.txt; then
        sed -i "1 s/\$/ cfg80211.ieee80211_regdom=$WIFI_COUNTRY/" /boot/firmware/cmdline.txt
    fi
fi

# SSH key for tests.
install -d -m 700 -o pi -g pi /home/pi/.ssh
cat /tmp/tc2-emu.pub >> /home/pi/.ssh/authorized_keys
chown pi:pi /home/pi/.ssh/authorized_keys
chmod 600 /home/pi/.ssh/authorized_keys

if emulator; then
    echo "built=$(date -Iseconds)" > /etc/tc2-emulator
else
    echo "built=$(date -Iseconds) saltops=$(awk '$1 == "commit" { print $2 }' /tmp/tc2-emu-guest/versions/saltops.txt)" > /etc/tc2-real-image
fi
