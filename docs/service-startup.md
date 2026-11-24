# Service startup: inventory and ordering findings

Started 2026-11-24 as the first step towards a deterministic, still parallel, startup
order. Raw data from `scripts/service-inventory.sh` on a freshly booted emulator
(registered, before any test): `docs/service-inventory-raw-2026-11-24.txt`.

## Boot timeline (seconds since power on, one fresh emulator boot)

| t | Event |
|---|---|
| 0 - 8.0 | kernel (TCG) |
| ~20 | multi-user.target reached (19.9s into userspace is the figure systemd prints) |
| 20 - 28 | event-reporter, tc2-hat-rtc, tc2-hat-i2c, ssh, NetworkManager, service-watcher, tc2-hat-temp |
| 31.9 | device-register registers the device (device-register is the slowest unit in `systemd-analyze blame`, 8.4s) |
| 33.9 - 35.4 | device-register starts what was skipped at boot: cacophony-config-sync, tc2-agent (34.3), tc2-hat-attiny, thermal-uploader, managementd.socket, tc2-hat-comms |
| 38.6 | tc2-agent exits (status 0) 4s after starting, see 1 below |
| 39.7 | thermal-recorder-py exits (config change at registration) and restarts |
| 43.8 | tc2-agent restarts after `RestartSec=5s`; frames flow from about here |

Total boot is 36.5s (8.0 kernel + 28.5 userspace), but the first recording needs tc2-agent
and thermal-recorder-py, which aren't both up until ~44s.

## Findings

1. **tc2-agent's first run always ends in a restart.** Its package postinst
   (`tc2-agent/_releases/scripts/postinst`) creates `/etc/cacophony/offload_erase`. On the
   first run the agent offloads, "erases" the RP2040 flash and exits
   (`program_rp2040.rs: erase_rp2040_flash_and_exit`; the emulated RP2040 just removes the
   file), and systemd restarts it after `RestartSec=5s`. The image has the file because the
   deb was installed during the image build, so every fresh emulator boot pays ~5s plus a
   second agent start. On a real device it's once per tc2-agent upgrade, as designed.
2. **thermal-recorder-py restarts once at registration**, because device-register writes
   `config.toml` while it starts. It's the config-watch behaviour in `open-findings.md`;
   its Python start is ~7s of CPU under TCG.
3. **Only managementd, tc2-hat-attiny and salt-minion wait for registration** (via
   `ConditionPathExists=/etc/salt/minion_id`), plus tc2-agent and cacophony-config-sync
   now (emulator drop-in, upstream patches in `upstream-patches/`). Everything else starts at
   boot regardless.
4. **Ordering is mostly coarse.** Only `tc2-hat-i2c` and `tc2-hat-rtc` say `Before=` the
   attiny unit. Services that call each other over D-Bus (org.cacophony.i2c, RTC, ATtiny,
   Events, TC2Agent, thermalrecorder, trap) don't order on the provider; they retry or fail
   instead ("dbus service not available within the timeout period", "Failed reading ready
   state from attiny", classifier's "Missing interface" for ATtiny).
5. **Several units say `After=multi-user.target` while `WantedBy=multi-user.target`**
   (tc2-hat-attiny, tc2-hat-comms, tc2-hat-temp, service-watcher, thermal-classifier, ...).
   In the journal these start after the target is reached. Whether that is deliberate
   deferral or an ordering cycle that systemd breaks isn't established.

## Services that start, stop or restart other services

Found by grepping the repos for `systemctl` and the like. None of these show in a unit file,
and they're why a slow or failed start in one service can take others down.

| Caller | Does | When |
|---|---|---|
| tc2-hat-attiny (`attiny.go: connectToATtinyWithRetries`, `updateATtinyFirmware`) | stops tc2-hat-comms, then stops tc2-agent, then erases and rewrites the ATtiny firmware over UART, then starts them again | on **any** failure to connect to the ATtiny, including "dbus service not available" because tc2-hat-i2c isn't answering yet. Up to 10 attempts |
| tc2-agent (`program_rp2040.rs`) | stops and restarts tc2-rp2040-logs | while programming the RP2040 |
| managementd (`api.go`) | restarts thermal-recorder-py, stops and starts tc2-agent | on test recordings, camera setup and firmware updates |
| tc2-hat-comms, thermal-recorder-py | exit when `config.toml` changes, relying on systemd to restart them | any config write |
| device-register | starts everything skipped at boot, restarts services waiting to restart | once, after registering |

If the ATtiny firmware update fails (`Error updating firmware: exit status 1`), `updateATtinyFirmware`
returns before its "Starting tc2-agent" step, so **tc2-agent stays stopped**. This is what happened
in the first failed full e2e run on 2026-11-24: i2c's dbus service wasn't up in time (the guest was
slow), tc2-hat-attiny read that as an absent ATtiny, stopped tc2-agent, failed to update, and
nothing started the agent again, so no frames flowed. The same sequence is possible on a real device
under load at boot. I've read the code and seen the log sequence once; I haven't reproduced it.

## Why the provider/consumer ordering matters

`tc2-hat-i2c` is `Type=simple`, so systemd considers it started when the process is, not when it
has the `org.cacophony.i2c` name. `tc2-hat-attiny` and `tc2-hat-rtc` both need that name, but only
`tc2-hat-i2c` orders itself before them (and `tc2-hat-rtc` has no order against i2c at all; the
code says "will error the first time as the I2C dbus interface is not yet up" and retries every 3s).
In the inventory, tc2-hat-rtc became active at 21.1s and tc2-hat-i2c at 22.5s.

## Proposed edges (1, 2 and 4 applied 2026-11-24; see status at the end of each)

These are chosen to add no waiting that the dependency doesn't already impose.

1. **`Type=dbus` + `BusName=` on the providers** (`tc2-hat-i2c`: org.cacophony.i2c, `tc2-hat-rtc`:
   org.cacophony.RTC, `tc2-hat-attiny`: org.cacophony.ATtiny, `event-reporter`:
   org.cacophony.Events, `tc2-agent`: org.cacophony.TC2Agent). systemd then treats a unit as
   started when it owns the name, with no code change. Each unit is an upstream edit in its repo.
2. **Consumers order after, and want, their providers:** `tc2-hat-rtc` and `tc2-hat-attiny` `After=` and
   `Wants=tc2-hat-i2c.service`; `tc2-agent` after `tc2-hat-attiny` (it needs ATtiny: "Failed reading
   ready state from attiny"); `tc2-agent` after `event-reporter`. Providers still start in parallel
   with each other and with everything unrelated; only a real dependency is waited for.
3. **Keep the registration gate** (`ConditionPathExists=/etc/salt/minion_id`) on tc2-agent and
   cacophony-config-sync (patches already drafted) and add it to anything else that only works once
   registered. Check thermal-uploader (it was started by device-register, so it has one).
4. **Don't let a transient dbus failure look like a missing ATtiny.** In tc2-hat-controller,
   `connectToATtiny` should only fall through to a firmware update when the ATtiny itself fails to
   answer, not when the dbus service is unavailable; and `updateATtinyFirmware` should start
   tc2-agent again on every exit path. This is a code change, and the most valuable one here.

The effect on boot time is untested. The estimate is that edges 1 and 2 remove retries (rtc's 3s sleep,
tc2-agent's 2s dbus retries) and so shorten time-to-frames, not lengthen it; that needs measuring with
`systemd-analyze critical-chain` and the inventory script before and after.

## D-Bus providers and consumers (from the repos, not yet checked on a device)

| Name | Provided by | Used by (found by grep) |
|---|---|---|
| org.cacophony.i2c | tc2-hat-i2c | tc2-agent, tc2-hat-controller, classifier-pipeline |
| org.cacophony.RTC | tc2-hat-rtc | managementd, tc2-hat-controller |
| org.cacophony.ATtiny | tc2-hat-attiny | tc2-agent, classifier-pipeline, tc2-hat-controller |
| org.cacophony.Events | event-reporter | tc2-agent, classifier-pipeline, managementd |
| org.cacophony.TC2Agent | tc2-agent | managementd |
| org.cacophony.thermalrecorder | thermal-recorder-py | managementd, tc2-hat-controller |
| org.cacophony.trap | tc2-hat-comms | managementd |

## Next

- Reproduce the failure behind proposed edge 4 (ATtiny firmware update on a transient dbus failure) deliberately, e.g. by
  delaying tc2-hat-i2c's start in the emulator, so the fix can be tested.

- Verify the table above by watching which calls fail during boot (dbus-monitor or the
  per-service retry logs), rather than from grep.
- For each unit: what must exist before it can do useful work (a dbus name, a file, the
  registration), which of those it retries, and what its start costs.
- Then propose edges (`After=`/`Requires=`/conditions) that remove retries and restarts,
  and check `systemd-analyze critical-chain` doesn't get longer.

## Status of the proposed edges (2026-11-24)

- **1 and 2, for the I2C chain only:** `tc2-hat-i2c` is `Type=dbus`; `tc2-hat-rtc` and `tc2-hat-attiny`
  are `After=`/`Wants=` it. Upstream patch `upstream-patches/tc2-hat-controller/0002`; emulator drop-ins
  in `guest-setup.sh`. On a fresh boot tc2-hat-rtc starts 0.1s after i2c owns its name and finds the RTC
  first time; the boot journal has none of "dbus service not available", "Error checking for PCF8563",
  "Failed to initialize attiny" or "Stopping tc2-agent". Boot 34.2s (7.5 kernel + 26.7 userspace), no
  longer than before. Full `make e2e` green, 6m53s.
- **Not done on purpose:** `Type=dbus` on tc2-hat-attiny (its start can include a firmware update that a
  start timeout would kill) and tc2-agent after tc2-hat-attiny (the agent starts before
  multi-user.target and attiny after it; ordering them would delay the agent on every boot). Whether the
  agent fails and restarts on a registered device's boot because attiny isn't up yet needs measuring with
  a reboot of a registered emulator.
- **4:** `guest/patches/tc2-hat-controller/0002-attiny-wait-for-i2c-service-before-reprogramming.patch`
  (built into the emulator's tc2-hat-controller). Unit-tested; the live check (take the I2C service away
  while tc2-hat-attiny starts) wasn't achieved: something restarts tc2-hat-i2c whenever it's stopped, not
  found yet.
