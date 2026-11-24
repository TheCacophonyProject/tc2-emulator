#!/bin/sh
# Dumps what's needed to map the guest's service startup dependencies, from a running
# emulator (run it over `make ssh`, or: `make ssh < scripts/service-inventory.sh`):
# for every unit multi-user.target pulls in, its ordering/requirement edges, start
# conditions and restart policy; when it became active relative to boot (monotonic
# seconds); and how often each one failed or was scheduled to restart this boot.
# Run it on a freshly booted emulator, before tests change config and restart things.
set -u
echo "### boot"
systemd-analyze 2>/dev/null
echo
echo "### units (name | ActiveEnter s | state | cond | Restart/RestartSec | After | Before | Requires | Wants)"
units=$(systemctl show --property=Wants --value multi-user.target basic.target | tr ' ' '\n' | sort -u)
for u in $units; do
    systemctl show "$u" -p Id -p ActiveState -p SubState -p ConditionResult \
        -p ActiveEnterTimestampMonotonic -p Restart -p RestartUSec -p After -p Before \
        -p Requires -p Wants -p NRestarts -p Result 2>/dev/null | awk -F= '
        { v[$1] = substr($0, length($1) + 2) }
        END {
            t = v["ActiveEnterTimestampMonotonic"] / 1000000
            printf "%s | %.1f | %s/%s result=%s | cond=%s | %s %s | n_restarts=%s\n  After: %s\n  Before: %s\n  Requires: %s\n  Wants: %s\n",
                v["Id"], t, v["ActiveState"], v["SubState"], v["Result"], v["ConditionResult"],
                v["Restart"], v["RestartUSec"], v["NRestarts"], v["After"], v["Before"], v["Requires"], v["Wants"]
        }'
done
echo
echo "### systemd-analyze blame (top 30)"
systemd-analyze blame 2>/dev/null | head -30
echo
echo "### failures and restarts this boot (journal, by unit)"
journalctl -b -o short-monotonic --no-pager 2>/dev/null |
    grep -E 'Failed with result|Scheduled restart job|Main process exited' |
    sed -E 's/^\[ *([0-9.]+)\] [^ ]+ systemd\[1\]: ([^:]+): (.*)$/\1 \2 \3/' | head -80
