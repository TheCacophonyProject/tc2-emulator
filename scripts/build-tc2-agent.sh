#!/usr/bin/env bash
# Builds tc2-agent for the guest with this repo's patches applied, like its
# release workflow does. Runs in the stubs builder container with the repo at
# /src (see the Makefile).
set -euo pipefail

# The release the image is updated to (see scripts/update-versions.sh).
VERSION=$(awk '$1 == "tc2-agent" { print $2 }' /src/guest/versions/packages.txt)

OUT=${1:?usage: build-tc2-agent.sh <tc2-agent output> [<tc2-rp2040-logs output>]}
# The same build also makes tc2-rp2040-logs, which the real-device image installs.
LOGS_OUT=${2:-}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/tc2-agent "$SRC"
git -C "$SRC" apply /src/guest/patches/tc2-agent/*.patch

# The tc2-firmware release whose firmware this release expects, as its release
# workflow sets it. The emulated RP2040 must report the same firmware version.
RP2040_FIRMWARE_RELEASE=$(sed -n 's/^ *RP2040_FIRMWARE_VERSION: *\([0-9.]*\)$/\1/p' "$SRC/.github/workflows/release.yaml")
: "${RP2040_FIRMWARE_RELEASE:?}"
echo "tc2-agent $VERSION expects tc2-firmware $RP2040_FIRMWARE_RELEASE"
mkdir -p "$SRC/_releases"
curl -sfL -o "$SRC/_releases/tc2-firmware.sha256" \
    "https://github.com/TheCacophonyProject/tc2-firmware/releases/download/v$RP2040_FIRMWARE_RELEASE/tc2-firmware.sha256"

cd "$SRC"
CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc \
    cargo build --release --target aarch64-unknown-linux-musl
install -D target/aarch64-unknown-linux-musl/release/tc2-agent "$OUT"
if [[ -n $LOGS_OUT ]]; then
    install -D target/aarch64-unknown-linux-musl/release/tc2-rp2040-logs "$LOGS_OUT"
fi
