#!/usr/bin/env bash
# Builds device-register for the guest with this repo's patches applied, against
# go-api with its patches applied, which sets the hostname when it registers.
# Runs in a golang container with the repo at /src (see the Makefile).
set -euo pipefail

# The release the image is updated to (see scripts/update-versions.sh).
VERSION=$(awk '$1 == "device-register" { print $2 }' /src/guest/versions/packages.txt)

OUT=${1:?usage: build-device-register.sh <output>}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/device-register "$SRC/device-register"
git -C "$SRC/device-register" apply /src/guest/patches/device-register/*.patch
cd "$SRC/device-register"
# go-api at the version device-register uses.
GO_API_VERSION=$(go list -m -f '{{.Version}}' github.com/TheCacophonyProject/go-api)
git clone -q --depth 1 --branch "$GO_API_VERSION" https://github.com/TheCacophonyProject/go-api "$SRC/go-api"
git -C "$SRC/go-api" apply /src/guest/patches/go-api/*.patch
go mod edit -replace "github.com/TheCacophonyProject/go-api=$SRC/go-api"
go mod tidy
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -o "$OUT" \
    -ldflags "-s -w -X main.version=$VERSION+tc2-emu" ./cmd/device-register
