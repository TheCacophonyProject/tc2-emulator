#!/usr/bin/env bash
# Builds event-reporter-tools (event-reporter, service-watcher, version-reporter, ...) for the
# guest with this repo's patches applied, with the same version and ldflags as the release
# installed in the image. Runs in a golang container with the repo at /src (see the Makefile).
set -euo pipefail

# The release the image is updated to (see scripts/update-versions.sh).
VERSION=$(awk '$1 == "event-reporter" { print $2 }' /src/guest/versions/packages.txt)

OUT=${1:?usage: build-event-reporter.sh <output>}
SRC=$(mktemp -d)
trap 'rm -rf "$SRC"' EXIT

git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/event-reporter "$SRC"
git -C "$SRC" apply /src/guest/patches/event-reporter/*.patch
cd "$SRC"
unformatted=$(gofmt -l ./internal ./cmd ./eventclient ./eventstore)
if [[ -n $unformatted ]]; then
    echo "gofmt would change: $unformatted" >&2
    exit 1
fi
go vet ./internal/service-watcher/
go test ./internal/service-watcher/
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -o "$OUT" -ldflags "-s -w -X main.version=$VERSION+tc2-emu" \
    ./cmd/event-reporter-tools
