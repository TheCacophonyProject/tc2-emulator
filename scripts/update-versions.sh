#!/usr/bin/env bash
# Pins guest/versions/packages.txt to the TC2 software versions salt deploys on
# a channel, from salt-version-info, and guest/versions/saltops.txt to the saltops
# commit of that channel's branch that salt-version-info was generated from, so
# the salt states the image is brought up to (guest/salt/apply-salt.sh) pin the
# same versions. The image is updated to these before the emulator's patches are
# applied (see guest-setup.sh and the build-*.sh scripts).
# The channel is the salt `environment` grain without its `tc2-` prefix. The
# emulator follows test, to try releases before they reach prod devices; the
# image's own grain is still tc2-prod, but there's no salt master to act on it.
set -euo pipefail

CHANNEL=${1:-test}
URL=https://raw.githubusercontent.com/TheCacophonyProject/salt-version-info/main/salt-version-info.json
OUT=$(dirname "$0")/../guest/versions/packages.txt
SALTOPS_OUT=$(dirname "$0")/../guest/versions/saltops.txt

curl -sfL "$URL" > "$OUT.json"
UPDATED=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["updated"])' "$OUT.json")
# The last commit on the channel's branch before salt-version-info was updated.
COMMIT=$(curl -sfL "https://api.github.com/repos/TheCacophonyProject/saltops/commits?sha=$CHANNEL&until=$UPDATED&per_page=1" |
    python3 -c 'import json, sys; print(json.load(sys.stdin)[0]["sha"])')
cat > "$SALTOPS_OUT" <<EOF2
# The saltops commit the image is brought up to, as a salt highstate would
# (guest/salt/apply-salt.sh). It's the $CHANNEL branch at the commit
# salt-version-info reflects, so its package pins are those of packages.txt.
# Regenerate with scripts/update-versions.sh.
commit $COMMIT
EOF2

python3 -c '
import json, sys
channel = sys.argv[1]
info = json.load(open(sys.argv[2]))
versions = info[channel]["tc2"]
print("# %s.tc2 from salt-version-info, updated %s." % (channel, info["updated"]))
print("# Regenerate with scripts/update-versions.sh.")
for package, version in sorted(versions.items()):
    if package != "commitDate":
        print(package, version)
' "$CHANNEL" "$OUT.json" > "$OUT.tmp"
rm "$OUT.json"
mv "$OUT.tmp" "$OUT"
cat "$OUT" "$SALTOPS_OUT"
