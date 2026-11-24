SRC_IMG ?= tc2-2026-02-03.img
IMAGES  := images
SSH     := ssh -i $(IMAGES)/id_ed25519 -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR pi@localhost

.PHONY: image real-image stubs stubs-builder build run up down e2e ssh console logs clean

image: $(IMAGES)/base.qcow2

$(IMAGES)/id_ed25519:
	mkdir -p $(IMAGES)
	ssh-keygen -t ed25519 -N '' -C tc2-emulator -f $@

$(IMAGES)/base.qcow2: $(SRC_IMG) $(IMAGES)/id_ed25519 scripts/prepare-image.sh scripts/guest-setup.sh builder/Dockerfile $(wildcard guest/*/*) $(wildcard guest/salt/fakebin/*)
	docker build -t tc2-emulator-builder builder
	docker run --rm --privileged -v /dev:/dev -v $(CURDIR):/work -w /work \
		-e HOST_UID=$(shell id -u) -e HOST_GID=$(shell id -g) \
		tc2-emulator-builder scripts/prepare-image.sh $(SRC_IMG) $(IMAGES)

# An image to write to an SD card for a real TC2, to compare it with the emulator (see
# the README): the same image and changes, but none of what's for QEMU, and the builds
# with guest/patches applied installed in place of the packages' binaries. Needs
# real-device.conf, from real-device.conf.example, for the wifi and the test API.
real-image: $(IMAGES)/tc2-real.img

$(IMAGES)/real_ed25519:
	mkdir -p $(IMAGES)
	ssh-keygen -t ed25519 -N '' -C tc2-real -f $@

real-device.conf:
	@echo "Copy real-device.conf.example to real-device.conf and fill it in." >&2; false

REAL_BINS := $(addprefix build/guest/,managementd tc2-agent tc2-rp2040-logs tc2-hat-controller device-register cacophony-config-utils tc2-tzlookup event-reporter-tools)

$(IMAGES)/tc2-real.img: $(SRC_IMG) $(IMAGES)/real_ed25519 real-device.conf scripts/prepare-image.sh scripts/guest-setup.sh builder/Dockerfile $(wildcard guest/*/*) $(wildcard guest/salt/fakebin/*) $(REAL_BINS)
	docker build -t tc2-emulator-builder builder
	docker run --rm --privileged -v /dev:/dev -v $(CURDIR):/work -w /work \
		-e HOST_UID=$(shell id -u) -e HOST_GID=$(shell id -g) -e REAL_CONF=/work/real-device.conf \
		tc2-emulator-builder scripts/prepare-image.sh $(SRC_IMG) $(IMAGES) device

# tc2-emu-* binaries: build/guest is shared into the guest over 9p, build/host
# runs in the container, so it's built for the Docker host's architecture
# (x86_64, or aarch64 on Apple silicon). Built in Docker with a separate target
# dir and cargo home, so they don't mix with host builds. install-replace.sh
# renames new binaries into place rather than overwriting them, so this works
# while the emulator is running them; it runs in Docker too, since macOS's
# install lacks -D and -t.
GUEST_BINS := tc2-emu-i2c tc2-emu-agent tc2-emu-dns
HOST_BINS  := tc2-emu-hw tc2-emu-rp2040
GUEST_TARGET := aarch64-unknown-linux-musl
HOST_TARGET   = $(shell docker info --format '{{.Architecture}}')-unknown-linux-musl
DOCKER_RUN := docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -w /src
CARGO_DOCKER := $(DOCKER_RUN) \
	-e CARGO_HOME=/src/target/docker/cargo-home -e CARGO_TARGET_DIR=/src/target/docker \
	tc2-emu-stubs-builder
INSTALL := $(DOCKER_RUN) tc2-emu-stubs-builder scripts/install-replace.sh

stubs-builder:
	docker build -q -t tc2-emu-stubs-builder -f docker/stubs.Dockerfile docker

stubs: | stubs-builder
	$(CARGO_DOCKER) cargo build --release --target $(GUEST_TARGET) $(addprefix -p ,$(GUEST_BINS))
	$(CARGO_DOCKER) cargo build --release --target $(HOST_TARGET) $(addprefix -p ,$(HOST_BINS))
	$(INSTALL) -D -t build/guest $(addprefix target/docker/$(GUEST_TARGET)/release/,$(GUEST_BINS))
	$(INSTALL) -D -t build/host $(addprefix target/docker/$(HOST_TARGET)/release/,$(HOST_BINS))
	$(INSTALL) -D -m 644 -t build/guest/fixtures fixtures/*.cptv
	$(INSTALL) -D -t build/guest guest/bin/*

# Fixture recordings aren't committed, since recordings from real cameras carry
# their location. tc2-emu-agent replaces it with a dummy one.
# tc2-firmware commit to take fixtures from.
FIXTURES_COMMIT := 300b2b99eb6e51eee1af57a603de23b599dd350a
stubs: fixtures/cat-trigger.cptv
fixtures/cat-trigger.cptv:
	curl -sfL --create-dirs -o $@ \
		https://raw.githubusercontent.com/TheCacophonyProject/tc2-firmware/$(FIXTURES_COMMIT)/test-fixtures/cat-trigger.cptv

# Released tc2-agent with guest/patches applied, until they're upstream. Uses the
# Docker cargo home for crates, but its own target dir inside the clone.
stubs: build/guest/tc2-agent build/guest/tc2-rp2040-logs
# One build makes both binaries; tc2-rp2040-logs is only used by the real-device image (the
# emulated RP2040 has no SWD for it to read logs over), so it runs there with the same patches.
build/guest/tc2-agent build/guest/tc2-rp2040-logs: scripts/build-tc2-agent.sh guest/versions/packages.txt $(wildcard guest/patches/tc2-agent/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e CARGO_HOME=/src/target/docker/cargo-home \
		tc2-emu-stubs-builder /src/scripts/build-tc2-agent.sh /src/target/docker/tc2-agent-bin /src/target/docker/tc2-rp2040-logs-bin
	$(INSTALL) -D target/docker/tc2-agent-bin build/guest/tc2-agent
	$(INSTALL) -D target/docker/tc2-rp2040-logs-bin build/guest/tc2-rp2040-logs

# The emulated RP2040: the tc2-firmware release the pinned tc2-agent ships, built on
# its mocked HAL with guest/patches/tc2-firmware, for the container.
stubs: build/host/tc2-firmware-emu
build/host/tc2-firmware-emu: scripts/build-tc2-firmware-emu.sh guest/versions/packages.txt $(wildcard guest/patches/tc2-firmware/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e CARGO_HOME=/src/target/docker/cargo-home \
		tc2-emu-stubs-builder /src/scripts/build-tc2-firmware-emu.sh $(HOST_TARGET) /src/target/docker/tc2-firmware-emu-bin
	$(INSTALL) -D target/docker/tc2-firmware-emu-bin build/host/tc2-firmware-emu

# The timezone lookup tc2-agent runs (guest/patches/tc2-agent 0002), a guest-local crate
# for now. Built on its own as well as with the tc2-emu-* binaries, since the real image
# needs it without the rest.
stubs: build/guest/tc2-tzlookup
build/guest/tc2-tzlookup: $(wildcard crates/tc2-tzlookup/* crates/tc2-tzlookup/src/*) Cargo.toml Cargo.lock | stubs-builder
	$(CARGO_DOCKER) cargo build --release --target $(GUEST_TARGET) -p tc2-tzlookup
	$(INSTALL) -D -t build/guest target/docker/$(GUEST_TARGET)/release/tc2-tzlookup

# Released tc2-hat-controller with guest/patches applied, until they're upstream.
stubs: build/guest/tc2-hat-controller
build/guest/tc2-hat-controller: scripts/build-tc2-hat-controller.sh guest/versions/packages.txt $(wildcard guest/patches/tc2-hat-controller/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e GOCACHE=/src/target/docker/go-cache -e GOMODCACHE=/src/target/docker/go-mod \
		golang:1.25-bookworm /src/scripts/build-tc2-hat-controller.sh /src/target/docker/tc2-hat-controller
	$(INSTALL) -D -t build/guest target/docker/tc2-hat-controller

# Released event-reporter (event-reporter-tools, which also runs service-watcher) with
# guest/patches applied, until they're upstream. Installed over the package's
# /usr/bin/event-reporter-tools (which event-reporter, service-watcher and version-reporter
# run) in the real image; or on a device with scripts/install-replace.sh or scp.
build/guest/event-reporter-tools: scripts/build-event-reporter.sh guest/versions/packages.txt $(wildcard guest/patches/event-reporter/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e GOCACHE=/src/target/docker/go-cache -e GOMODCACHE=/src/target/docker/go-mod \
		golang:1.25-bookworm /src/scripts/build-event-reporter.sh /src/target/docker/event-reporter-tools
	$(INSTALL) -D -t build/guest target/docker/event-reporter-tools

# Released go-config with guest/patches applied, for cacophony-config-sync, until
# they're upstream.
stubs: build/guest/cacophony-config-utils
build/guest/cacophony-config-utils: scripts/build-go-config.sh guest/versions/packages.txt $(wildcard guest/patches/go-config/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e GOCACHE=/src/target/docker/go-cache -e GOMODCACHE=/src/target/docker/go-mod \
		golang:1.25-bookworm /src/scripts/build-go-config.sh /src/target/docker/cacophony-config-utils
	$(INSTALL) -D -t build/guest target/docker/cacophony-config-utils

# Released managementd with guest/patches applied, built against the patched
# go-config above, so that it handles the config it adds, as for Sidekick, and
# the patched go-api, which writes the config when managementd reregisters.
stubs: build/guest/managementd
build/guest/managementd: scripts/build-managementd.sh guest/versions/packages.txt $(wildcard guest/patches/go-config/*) $(wildcard guest/patches/go-api/*) $(wildcard guest/patches/management-interface/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e npm_config_cache=/src/target/docker/npm-cache \
		node:24-bookworm /src/scripts/build-managementd.sh ts /src/target/docker/managementd-src
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e GOCACHE=/src/target/docker/go-cache -e GOMODCACHE=/src/target/docker/go-mod \
		-e GOPATH=/src/target/docker/go-path -e GOFLAGS=-buildvcs=false \
		golang:1.25-bookworm /src/scripts/build-managementd.sh go /src/target/docker/managementd-src /src/target/docker/managementd
	$(INSTALL) -D -t build/guest target/docker/managementd

# Released device-register with guest/patches applied, built against the patched
# go-api, so that it starts the services that need a registered device rather
# than rebooting, until they're upstream.
stubs: build/guest/device-register
build/guest/device-register: scripts/build-device-register.sh guest/versions/packages.txt $(wildcard guest/patches/go-api/*) $(wildcard guest/patches/device-register/*) | stubs-builder
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e GOCACHE=/src/target/docker/go-cache -e GOMODCACHE=/src/target/docker/go-mod \
		-e GOFLAGS=-buildvcs=false \
		golang:1.25-bookworm /src/scripts/build-device-register.sh /src/target/docker/device-register
	$(INSTALL) -D -t build/guest target/docker/device-register

build:
	docker compose build

# Foreground, serial console on stdout.
run: build stubs
	docker compose run --rm --service-ports tc2

up: build stubs
	docker compose up -d

down:
	docker compose down

# Starts a fresh emulator (a new, unregistered device) and runs the end-to-end
# tests against it and the API at localhost:1080. The emulator is left running.
# The tests reach both through the Docker host's published ports, since Docker
# Desktop only has host networking as an opt-in.
E2E_HOST := http://host.docker.internal
# The tests share the one emulated device, so they run one at a time, in this
# order: new_device_setup needs the device as it first registers, and the others
# move it to a group of their own.
E2E_TESTS := new_device_setup rp2040_test_recording config_sync power_mode classify
# cacophony-config-sync waits only 100ms after config changes, rather than 10s.
# ssh, which the tests use to run commands in the guest, needs the user the container
# runs as (the host user's id) to be in /etc/passwd.
e2e: build stubs
	mkdir -p target/docker
	printf 'root:x:0:0:root:/root:/bin/sh\ntc2:x:%s:%s::/tmp:/bin/sh\n' $(shell id -u) $(shell id -g) > target/docker/passwd
	TC2_CONFIG_SYNC_DELAY=100ms TC2_AUTOLOGIN=0 docker compose up -d --force-recreate
	$(DOCKER_RUN) --add-host host.docker.internal:host-gateway -v $(CURDIR)/target/docker/passwd:/etc/passwd:ro \
		-e TC2_E2E_API=$(E2E_HOST):1080 -e TC2_E2E_MANAGEMENTD=$(E2E_HOST):8080 \
		-e TC2_E2E_SSH=host.docker.internal:2222 -e TC2_E2E_SSH_KEY=/src/images/id_ed25519 \
		-e CARGO_HOME=/src/target/docker/cargo-home -e CARGO_TARGET_DIR=/src/target/docker \
		tc2-emu-stubs-builder sh -ec 'for test in $(E2E_TESTS); do \
			cargo test -p tc2-emu-e2e --test $$test -- --ignored --nocapture --test-threads=1; done'

ssh:
	$(SSH)

# Detach with ctrl-p ctrl-q.
console:
	docker compose attach tc2

# The guest's journal for this run, from boot, following. Each guest boot starts
# with a "-- Boot <id> --" line.
logs:
	docker compose exec tc2 tail -n +1 -F /state/journal.log

clean:
	rm -f $(IMAGES)/base.qcow2 $(IMAGES)/vmlinuz $(IMAGES)/initrd.img $(IMAGES)/work.img
