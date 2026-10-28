#!/usr/bin/env bash
# Builds tc2-hat-controller for the guest with this repo's patches applied, using
# the same version and ldflags as the release installed in the image. Runs in a
# golang container with the repo at /src (see the Makefile).
set -euo pipefail

# The release the image is updated to (see scripts/update-versions.sh).
VERSION=$(awk '$1 == "tc2-hat-controller" { print $2 }' /src/guest/versions/packages.txt)

OUT=${1:?usage: build-tc2-hat-controller.sh <output>}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/tc2-hat-controller "$SRC"

# The ATtiny firmware the release expects, as its release workflow sets it. The
# emulated ATtiny (tc2-emu-hw) must report the same version.
attiny_env() {
    sed -n "s/^ *$1: *\([0-9]*\)\$/\1/p" "$SRC/.github/workflows/goreleaser.yml"
}
ATTINY_MAJOR=$(attiny_env ATTINY_MAJOR)
ATTINY_MINOR=$(attiny_env ATTINY_MINOR)
ATTINY_PATCH=$(attiny_env ATTINY_PATCH)
ATTINY_HASH=$(curl -sfL "https://github.com/TheCacophonyProject/attiny1616/releases/download/v$ATTINY_MAJOR.$ATTINY_MINOR.$ATTINY_PATCH/firmware.hex.sha256" | cut -d ' ' -f 1)
: "${ATTINY_MAJOR:?}${ATTINY_MINOR:?}${ATTINY_PATCH:?}${ATTINY_HASH:?}"
echo "tc2-hat-controller $VERSION expects ATtiny firmware $ATTINY_MAJOR.$ATTINY_MINOR.$ATTINY_PATCH ($ATTINY_HASH)"
git -C "$SRC" apply /src/guest/patches/tc2-hat-controller/*.patch
cd "$SRC"
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -o "$OUT" -ldflags "-s -w \
    -X main.version=$VERSION+tc2-emu \
    -X main.attinyMajorStr=$ATTINY_MAJOR -X main.attinyMinorStr=$ATTINY_MINOR \
    -X main.attinyPatchStr=$ATTINY_PATCH -X main.attinyHexHash=$ATTINY_HASH" \
    ./cmd/tc2-hat-controller
