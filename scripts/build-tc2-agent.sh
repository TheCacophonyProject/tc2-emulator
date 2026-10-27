#!/usr/bin/env bash
# Builds tc2-agent for the guest with this repo's patches applied, like its
# release workflow does. Runs in the stubs builder container with the repo at
# /src (see the Makefile).
set -euo pipefail

COMMIT=812f7b1  # main, expecting RP2040 firmware version 40
RP2040_FIRMWARE_RELEASE=0.7.0  # tc2-firmware release with firmware version 40

OUT=${1:?usage: build-tc2-agent.sh <output>}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q https://github.com/TheCacophonyProject/tc2-agent "$SRC"
git -C "$SRC" checkout -q "$COMMIT"
git -C "$SRC" apply /src/guest/patches/tc2-agent/*.patch
mkdir -p "$SRC/_releases"
curl -sfL -o "$SRC/_releases/tc2-firmware.sha256" \
    "https://github.com/TheCacophonyProject/tc2-firmware/releases/download/v$RP2040_FIRMWARE_RELEASE/tc2-firmware.sha256"

cd "$SRC"
CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc \
    cargo build --release --target aarch64-unknown-linux-musl
install -D target/aarch64-unknown-linux-musl/release/tc2-agent "$OUT"
