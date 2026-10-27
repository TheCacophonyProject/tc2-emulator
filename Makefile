SRC_IMG ?= tc2-2026-02-03.img
IMAGES  := images
SSH     := ssh -i $(IMAGES)/id_ed25519 -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR pi@localhost

.PHONY: image stubs build run up down e2e ssh console clean

image: $(IMAGES)/base.qcow2

$(IMAGES)/id_ed25519:
	mkdir -p $(IMAGES)
	ssh-keygen -t ed25519 -N '' -C tc2-emulator -f $@

$(IMAGES)/base.qcow2: $(SRC_IMG) $(IMAGES)/id_ed25519 scripts/prepare-image.sh scripts/guest-setup.sh builder/Dockerfile $(wildcard guest/*/*)
	docker build -t tc2-emulator-builder builder
	docker run --rm --privileged -v /dev:/dev -v $(CURDIR):/work -w /work \
		-e HOST_UID=$(shell id -u) -e HOST_GID=$(shell id -g) \
		tc2-emulator-builder scripts/prepare-image.sh $(SRC_IMG) $(IMAGES)

# tc2-emu-* binaries: build/guest is shared into the guest over 9p, build/host
# runs in the container. Built in Docker with a separate target dir and cargo
# home, so they don't mix with host builds. `install` replaces the binaries rather
# than overwriting them, so this works while the emulator is running them.
GUEST_BINS := tc2-emu-i2c tc2-emu-agent
HOST_BINS  := tc2-emu-hw
CARGO_DOCKER := docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -w /src \
	-e CARGO_HOME=/src/target/docker/cargo-home -e CARGO_TARGET_DIR=/src/target/docker \
	tc2-emu-stubs-builder

stubs:
	docker build -q -t tc2-emu-stubs-builder -f docker/stubs.Dockerfile docker
	$(CARGO_DOCKER) cargo build --release --target aarch64-unknown-linux-musl $(addprefix -p ,$(GUEST_BINS))
	$(CARGO_DOCKER) cargo build --release --target x86_64-unknown-linux-musl $(addprefix -p ,$(HOST_BINS))
	install -D -t build/guest $(addprefix target/docker/aarch64-unknown-linux-musl/release/,$(GUEST_BINS))
	install -D -t build/host $(addprefix target/docker/x86_64-unknown-linux-musl/release/,$(HOST_BINS))
	install -D -m 644 -t build/guest/fixtures fixtures/*.cptv

# tc2-agent main with guest/patches applied, until they're upstream. Uses the
# Docker cargo home for crates, but its own target dir inside the clone.
stubs: build/guest/tc2-agent
build/guest/tc2-agent: scripts/build-tc2-agent.sh $(wildcard guest/patches/tc2-agent/*)
	docker build -q -t tc2-emu-stubs-builder -f docker/stubs.Dockerfile docker
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e CARGO_HOME=/src/target/docker/cargo-home \
		tc2-emu-stubs-builder /src/scripts/build-tc2-agent.sh /src/target/docker/tc2-agent-bin
	install -D target/docker/tc2-agent-bin build/guest/tc2-agent

# Released tc2-hat-controller with guest/patches applied, until they're upstream.
stubs: build/guest/tc2-hat-controller
build/guest/tc2-hat-controller: scripts/build-tc2-hat-controller.sh $(wildcard guest/patches/tc2-hat-controller/*)
	docker run --rm -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -e HOME=/tmp \
		-e GOCACHE=/src/target/docker/go-cache -e GOMODCACHE=/src/target/docker/go-mod \
		golang:1.25-bookworm /src/scripts/build-tc2-hat-controller.sh /src/target/docker/tc2-hat-controller
	install -D -t build/guest target/docker/tc2-hat-controller

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
e2e: build stubs
	docker compose up -d --force-recreate
	docker run --rm --network host -u $(shell id -u):$(shell id -g) -v $(CURDIR):/src -w /src \
		-e CARGO_HOME=/src/target/docker/cargo-home -e CARGO_TARGET_DIR=/src/target/docker \
		tc2-emu-stubs-builder cargo test -p tc2-emu-e2e -- --ignored --nocapture

ssh:
	$(SSH)

# Detach with ctrl-p ctrl-q.
console:
	docker compose attach tc2

clean:
	rm -f $(IMAGES)/base.qcow2 $(IMAGES)/vmlinuz $(IMAGES)/initrd.img $(IMAGES)/work.img
