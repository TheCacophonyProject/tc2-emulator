#!/usr/bin/env bash
# Brings the image up to what a salt highstate deploys, as the real devices'
# salt minions do: runs the TC2 states from the saltops commit pinned in
# guest/versions/saltops.txt, masterless, inside the image's chroot (see
# scripts/guest-setup.sh). The older image is missing what saltops has changed
# since it was made: files, models, service enablement.
#
# There's no systemd in the chroot, so guest/salt/fakebin stands in for the commands
# that need it. States that can't have their effect there are listed in
# expected-failures.txt; anything else failing, or a package state having to
# change a version (packages.txt and the pinned saltops commit have drifted
# apart), fails the build.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
COMMIT=$(awk '$1 == "commit" { print $2 }' "$HERE/../versions/saltops.txt")
WORK=/tmp/tc2-emu-salt
rm -rf "$WORK"
mkdir -p "$WORK/conf"

echo "Fetching saltops $COMMIT"
curl -sfL "https://github.com/TheCacophonyProject/saltops/archive/$COMMIT.tar.gz" | tar -xz -C "$WORK"
SALTOPS=$WORK/saltops-$COMMIT

cat > "$WORK/conf/minion" <<EOC
file_client: local
id: pi-0
state_top: tc2-top.sls
file_roots:
  base:
    - $SALTOPS
cachedir: $WORK/cache
log_file: $WORK/salt.log
log_level_logfile: info
log_level: warning
EOC
cp /etc/salt/grains "$WORK/conf/grains"

# Not run in a chroot with no systemd: salt's reboot after changing boot config,
# and the node group file, which the image's tc2-prod matches its environment
# grain and is what the salt-updater channel is chosen by.
EXCLUDE="tc2.maybe-reboot,tc2/maybe-reboot,tc2.dev,tc2/dev"

# Salt uses its systemd service states only where systemd is booted, which it
# tells by this directory, and then runs our systemctl.
mkdir -p /run/systemd/system
# Commands salt runs by absolute path, which the PATH can't redirect. Setting up
# swap would do it on the machine building the image, which shares the kernel
# with the chroot.
ABSOLUTE_STUBS=(/sbin/dphys-swapfile)
restore() {
    rm -rf /run/systemd /run/tc2-emu-fake-systemd
    for bin in "${ABSOLUTE_STUBS[@]}"; do
        [[ ! -e $bin.tc2-emu-real ]] || mv -f "$bin.tc2-emu-real" "$bin"
    done
}
trap restore EXIT
for bin in "${ABSOLUTE_STUBS[@]}"; do
    mv "$bin" "$bin.tc2-emu-real"
    cp "$HERE/fakebin/noop" "$bin"
done
export PATH=$HERE/fakebin:$PATH
export SALT_RESULT=$WORK/result.json
/usr/bin/salt-call --local -c "$WORK/conf" saltutil.sync_all >/dev/null
/usr/bin/salt-call --local -c "$WORK/conf" state.highstate "exclude=$EXCLUDE" \
    --out=json --out-file="$SALT_RESULT" || true

python3 - "$SALT_RESULT" "$HERE/expected-failures.txt" <<'EOP'
import json, sys

result = json.load(open(sys.argv[1]))["local"]
expected = [l.split("#")[0].strip() for l in open(sys.argv[2])]
expected = {e for e in expected if e}
if isinstance(result, list):
    sys.exit("highstate failed to render:\n" + "\n".join(result))

changed, failed, drifted = [], [], []
for key, state in sorted(result.items(), key=lambda kv: kv[1]["__run_num__"]):
    module, sid, name, fun = key.split("_|-")
    label = "%s.%s %s" % (module, fun, name.splitlines()[0][:70])
    if state["result"] is False:
        failed.append((label, state.get("comment", "").strip().splitlines()[:1]))
    if state["changes"]:
        changed.append(label)
        if module == "cacophony" and fun.startswith("pkg_installed"):
            drifted.append(label)
print("== states that changed something (%d of %d):" % (len(changed), len(result)))
for c in changed:
    print("  ", c)
unexpected = [f for f in failed if not any(e in f[0] for e in expected)]
print("== failed, expected in a chroot (%d):" % (len(failed) - len(unexpected)))
for f in failed:
    if f not in unexpected:
        print("  ", f[0])
if unexpected or drifted:
    print("== FAILED unexpectedly:")
    for f in unexpected:
        print("  ", f[0], "|", " ".join(f[1]))
    for d in drifted:
        print("   package changed version, update guest/versions:", d)
    sys.exit(1)
EOP
rm -rf "$WORK"
