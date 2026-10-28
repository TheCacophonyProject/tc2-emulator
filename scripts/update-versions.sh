#!/usr/bin/env bash
# Pins guest/versions/packages.txt to the TC2 software versions salt deploys on
# a channel, from salt-version-info. The image is updated to these before the
# emulator's patches are applied (see guest-setup.sh and the build-*.sh scripts).
# The channel is the salt `environment` grain without its `tc2-` prefix; the
# image's is tc2-prod.
set -euo pipefail

CHANNEL=${1:-prod}
URL=https://raw.githubusercontent.com/TheCacophonyProject/salt-version-info/main/salt-version-info.json
OUT=$(dirname "$0")/../guest/versions/packages.txt

curl -sfL "$URL" | python3 -c '
import json, sys
channel = sys.argv[1]
info = json.load(sys.stdin)
versions = info[channel]["tc2"]
print("# %s.tc2 from salt-version-info, updated %s." % (channel, info["updated"]))
print("# Regenerate with scripts/update-versions.sh.")
for package, version in sorted(versions.items()):
    if package != "commitDate":
        print(package, version)
' "$CHANNEL" > "$OUT.tmp"
mv "$OUT.tmp" "$OUT"
cat "$OUT"
