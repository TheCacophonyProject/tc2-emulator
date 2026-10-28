#!/usr/bin/env bash
# Builds go-config's cacophony-config-utils for the guest with this repo's patches
# applied, for cacophony-config-sync. Runs in a golang container with the repo at
# /src (see the Makefile).
set -euo pipefail

# The release the image is updated to (see scripts/update-versions.sh).
VERSION=$(awk '$1 == "cacophony-config" { print $2 }' /src/guest/versions/packages.txt)

OUT=${1:?usage: build-go-config.sh <output>}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/go-config "$SRC"
git -C "$SRC" apply /src/guest/patches/go-config/*.patch
cd "$SRC"
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -o "$OUT" \
    -ldflags "-s -w -X main.version=$VERSION+tc2-emu" ./cmd/cacophony-config-utils
