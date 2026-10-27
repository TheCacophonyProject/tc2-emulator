# tc2-emulator

Runs a TC2 thermal camera SD card image under QEMU in Docker, for end-to-end tests.

The Pi userspace runs unchanged on QEMU's `virt` machine (Cortex-A53, 4 cores,
512M to match the Pi Zero 2 W) with a Debian generic arm64 kernel, since the Pi
kernel has no virtio drivers. Pi/HAT hardware (SPI, I2C, GPIO, RP2040, ATtiny)
is not emulated.

## Usage

```sh
make image    # tc2-2026-02-03.img -> images/base.qcow2 + kernel (needs privileged docker)
make stubs    # build the tc2-emu-* binaries into build/ (also run by `make up`/`make run`)
make up       # boot in the background (or `make run` for the console in the foreground)
make ssh      # ssh pi@ the guest using images/id_ed25519
make console  # serial console; detach with ctrl-p ctrl-q
make down
```

Use a different source image with `make image SRC_IMG=path/to.img`.

- SSH: `localhost:2222`, management-interface: `http://localhost:8080`,
  hardware control API: `http://localhost:9000`
- Each run starts from a clean copy of `base.qcow2`. `TC2_PERSIST=1` keeps changes.
- `make image` registers an arm64 binfmt handler on the host kernel if missing
  (lasts until reboot).

## Guest changes made by `make image`

See `scripts/guest-setup.sh`. In short:

- `device-register` registers with the test API instead of production (see below).
- `salt-minion`, `salt-updater`, swap, `rpi-eeprom-update` and `sshswitch` are
  disabled; add `tc2-emu.enable=<service>` to the kernel command line to enable one.
- The first-boot rootfs expansion in `rc.local` is skipped.
- Serial console autologin as `pi`, plus a test SSH key.

## Networking and the test API

The image configures `eth0` statically as `192.168.1.2/24` with no gateway. QEMU's
user network uses that subnet, with the host side at `192.168.1.1`. The guest has
no route to the internet, so it can't reach production servers.

`http://192.168.1.1:1080` in the guest is relayed to `TC2_API` (default
`host.docker.internal:1080`, i.e. port 1080 on the Docker host).

Services wait until modemd's connection check passes (ping 8.8.8.8/8.8.4.4 and
resolve google.com). The emulator answers it locally: a dummy interface holds
those addresses and the guest's dnsmasq resolves every name to `192.168.1.1`.

## Registration

On a fresh device `device-register` runs (no `/etc/salt/minion_id` yet), registers
with the test API, writes `minion_id` as `pi-<device id>` and reboots. After that
`managementd` (the web UI) starts.

```sh
TC2_GROUP=my-group TC2_NAME=my-device make up   # TC2_NAME defaults to a random name
```

The group must already exist on the test API. Each run starts from a fresh disk
and registers a new device; use `TC2_PERSIST=1` to keep the registration.

## Emulated hardware

The Rust workspace in `crates/` stands in for the TC2 HAT:

- `tc2-emu-hw` runs in the container and models the devices on the HAT's I2C bus:
  the ATtiny power controller (0x25, firmware v1.0.9), PCF8563 RTC (0x51), AHT20
  temperature/humidity sensor (0x38) and the hardware EEPROM (0x50, power PCB v0.7.0).
- `tc2-emu-i2c` replaces `tc2-hat-i2c` in the guest. All TC2 software reaches the
  I2C bus through its `org.cacophony.i2c.Tx` D-Bus method, which it forwards to
  `tc2-emu-hw` over a virtio-serial port. The real `tc2-hat-attiny`, `tc2-hat-rtc`
  and `tc2-hat-temp` run unchanged on top.

Guest binaries are cross-compiled for `aarch64-unknown-linux-musl` and shared into
the guest at `/opt/tc2-emu` over 9p, so changing a stub needs `make stubs` and a
restart, not a new image.

Tests change the hardware through the control API:

```sh
curl localhost:9000/state
curl -X PATCH localhost:9000/attiny -H content-type:application/json -d '{"hv_battery_volts": 11.2}'
curl -X PATCH localhost:9000/aht20 -H content-type:application/json -d '{"temperature_celsius": 35}'
curl -X PUT localhost:9000/rtc/time -H content-type:application/json -d '{"time": "2027-01-01T09:00:00Z"}'
```

The ATtiny reports that the RP2040 wants the Pi on (`rp2040_wants_pi_on`) by
default. Without that, `tc2-hat-attiny` powers the Pi off once its startup grace
period ends and nothing else has asked it to stay on.

GPIO comes from `gpio-mockup` (see `guest/gpio-mockup/`): one chip at base 0, so
`GPIO0`-`GPIO27` match the Pi's numbering. Inputs are set through
`/sys/kernel/debug/gpio-mockup/gpiochip0/<line>`.
