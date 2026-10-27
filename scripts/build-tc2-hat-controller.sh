#!/usr/bin/env bash
# Builds tc2-hat-controller for the guest with this repo's patches applied, using
# the same version and ldflags as the release installed in the image. Runs in a
# golang container with the repo at /src (see the Makefile).
set -euo pipefail

VERSION=0.25.0
# From `tc2-hat-attiny --attiny-firmware-info` on the image.
ATTINY_MAJOR=1
ATTINY_MINOR=0
ATTINY_PATCH=9
ATTINY_HASH=0ab3e8cbd00a3e47a039dced0909927cbebe5b36e82e299d8e34e875785dbabd

OUT=${1:?usage: build-tc2-hat-controller.sh <output>}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/tc2-hat-controller "$SRC"
git -C "$SRC" apply /src/guest/patches/tc2-hat-controller/*.patch
cd "$SRC"
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -o "$OUT" -ldflags "-s -w \
    -X main.version=$VERSION+tc2-emu \
    -X main.attinyMajorStr=$ATTINY_MAJOR -X main.attinyMinorStr=$ATTINY_MINOR \
    -X main.attinyPatchStr=$ATTINY_PATCH -X main.attinyHexHash=$ATTINY_HASH" \
    ./cmd/tc2-hat-controller
