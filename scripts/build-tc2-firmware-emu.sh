#!/usr/bin/env bash
# Builds tc2-firmware-emu, the emulated RP2040: the tc2-firmware release the pinned
# tc2-agent ships, with guest/patches/tc2-firmware applied (they add the emulator
# build of the firmware on its mocked HAL). Runs in the stubs builder container with
# the repo at /src (see the Makefile).
set -euo pipefail

TARGET=${1:?usage: build-tc2-firmware-emu.sh <target> <output>}
OUT=${2:?usage: build-tc2-firmware-emu.sh <target> <output>}

# The firmware release the pinned tc2-agent expects, as its release workflow sets it
# (see build-tc2-agent.sh).
AGENT_VERSION=$(awk '$1 == "tc2-agent" { print $2 }' /src/guest/versions/packages.txt)
VERSION=$(curl -sfL "https://raw.githubusercontent.com/TheCacophonyProject/tc2-agent/v$AGENT_VERSION/.github/workflows/release.yaml" \
    | sed -n 's/^ *RP2040_FIRMWARE_VERSION: *\([0-9.]*\)$/\1/p')
: "${VERSION:?no RP2040_FIRMWARE_VERSION in the release workflow of tc2-agent $AGENT_VERSION}"
echo "tc2-agent $AGENT_VERSION ships tc2-firmware $VERSION"

SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT
git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/tc2-firmware "$SRC"
git -C "$SRC" apply /src/guest/patches/tc2-firmware/*.patch

cd "$SRC"
# rust-toolchain.toml also asks for the RP2040's target, which this build doesn't need.
export RUSTUP_TOOLCHAIN=1.90.0
# The emulator's own tests (the link to tc2-agent etc.), not the firmware's.
cargo test --locked --target "$TARGET" --no-default-features --features emulator \
    --bin tc2-firmware-emu emulator::
cargo build --release --locked --target "$TARGET" --no-default-features --features emulator \
    --bin tc2-firmware-emu
install -D "target/$TARGET/release/tc2-firmware-emu" "$OUT"
