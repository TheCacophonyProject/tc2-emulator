#!/usr/bin/env bash
# Builds management-interface's managementd for the guest against go-config with
# this repo's patches applied, so that it reads and writes the config sections
# they add, as Sidekick uses it to. Runs in two steps, with the repo at /src (see
# the Makefile):
#   build-managementd.sh ts <dir>          in a node container: clones the release
#                                          into <dir> and compiles its TypeScript.
#   build-managementd.sh go <dir> <output> in a golang container: builds it with
#                                          the patched go-config, packing in the
#                                          web UI.
set -euo pipefail

# The releases the image is updated to (see scripts/update-versions.sh).
version() { awk -v p="$1" '$1 == p { print $2 }' /src/guest/versions/packages.txt; }
VERSION=$(version management-interface)
GO_CONFIG_VERSION=$(version cacophony-config)

STEP=${1:?usage: build-managementd.sh ts|go <dir> [<output>]}
DIR=${2:?usage: build-managementd.sh ts|go <dir> [<output>]}

case $STEP in
ts)
    rm -rf "$DIR"
    git clone -q --depth 1 --branch "v$VERSION" https://github.com/TheCacophonyProject/management-interface "$DIR/management-interface"
    cd "$DIR/management-interface"
    npm install -q --no-audit --no-fund --no-save typescript@5 @types/jquery
    npx tsc
    ;;
go)
    OUT=${3:?usage: build-managementd.sh go <dir> <output>}
    git clone -q --depth 1 --branch "v$GO_CONFIG_VERSION" https://github.com/TheCacophonyProject/go-config "$DIR/go-config"
    git -C "$DIR/go-config" apply /src/guest/patches/go-config/*.patch
    cd "$DIR/management-interface"
    go mod edit -replace "github.com/TheCacophonyProject/go-config=$DIR/go-config"
    go mod tidy
    go install github.com/gobuffalo/packr/packr@v1.30.1
    CGO_ENABLED=0 GOOS=linux GOARCH=arm64 "$(go env GOPATH)/bin/packr" build -o "$OUT" \
        -ldflags "-s -w -X main.version=$VERSION+tc2-emu" ./cmd/managementd
    ;;
*)
    echo "unknown step: $STEP" >&2
    exit 2
    ;;
esac
