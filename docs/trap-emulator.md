# Trap emulator design

Status: draft, 2026-11-09. Nothing here is implemented yet.

Emulates a Cacophony trap (a spool PCB running spool-py) on the TC2's UART, so
the real `tc2-hat-comms` can be tested end to end: from trap settings on the
API, through config sync, to the trap being enabled and its events reaching the
API.

## Background

`tc2-hat-comms` (tc2-hat-controller, `internal/tc2-hat-comms`) is the only user
of the HAT's UART. `[comms] CommsOut` picks one output mode:

| Mode | Baud | Use |
|---|---|---|
| `simple` | - | UART TX pin driven as a GPIO, high while the trap should be active |
| `json-out` | 115200 | One JSON line per classification, for third-party devices |
| `trap-control` (or `uart`) | 9600 | Two-way control of a trap running spool-py |
| `at-esl` | 4800 | AT commands to an AT-ESL node |

This design covers `trap-control`. Trap settings from the portal (`[trap]`)
force that mode when the trap is enabled (`applyTrapSettings`, our local
change in `upstream/tc2-hat-controller`, not yet upstream).

In `trap-control`, `tc2-hat-comms`:

1. PINGs until the trap answers.
2. Uploads `/etc/cacophony/mpy` (spool-py's compiled `.mpy` files, shipped in the
   tc2-hat-controller package) and `trap.Config` as `/config.json`, skipping
   files whose hash already matches, then COMMITs.
3. RESTARTs the trap if anything changed, and PINGs again.
4. Sets the trap's time, once the camera's clock has synced with NTP.
5. Serves `org.cacophony.trap` on D-Bus (status, restart, manual spool/door/cage
   requests) for management-interface.
6. Loops on `org.cacophony.thermalrecorder` `Tracking` and `Recording` signals,
   sending ENABLE/DISABLE as protect and target species are seen
   (`trapState`), renewing ENABLE every minute while enabled.
7. Turns messages the trap sends on its own into events for event-reporter.

The camera restarts `tc2-hat-comms` whenever a comms-relevant value in
`config.toml` changes, so a config sync becomes a fresh connection to the trap.

## Where it runs

Like `tc2-emu-hw` and `tc2-emu-rp2040`: in the container next to QEMU, outside
the guest, connected to a QEMU chardev socket. Its control API is published to
the host for tests.

```
 host / tests ──HTTP──► :9001 control API
                              │
 ┌─────────── container ──────┼──────────────────────────────┐
 │  tc2-emu-hw  tc2-emu-rp2040   tc2-emu-trap ◄── $STATE/trap.sock
 │                                     ▲  (raw bytes)        │
 │  qemu ── -chardev socket,id=trap ───┘                     │
 │   └── guest: tc2-hat-comms ─► /dev/serial0 ─► 16550 UART  │
 └───────────────────────────────────────────────────────────┘
```

Opt-in, like `TC2_AGENT=real`: `TC2_TRAP=1` starts `tc2-emu-trap` and adds the
UART to QEMU. Without it the guest has no `/dev/serial0`, as now.

## Guest UART

`serialhelper` hardcodes `/dev/serial0` and opens it with `tarm/serial`, which
sets the baud rate with termios ioctls. virtio-serial ports don't support
termios, so the virtio-serial pattern used for I2C and the RP2040 won't do.

**Chosen: QEMU `pci-serial`** (a 16550 on PCI) backed by a socket chardev:

```
-chardev socket,id=trap,path="$STATE/trap.sock",server=on,wait=off
-device pci-serial,chardev=trap
```

It appears as `/dev/ttyS*` with real termios; a udev rule in `guest-setup.sh`
links `/dev/serial0` to it. The Debian arm64 kernel is expected to have
`CONFIG_SERIAL_8250_PCI`; confirm in T1. The baud rate isn't carried over the
socket, so the emulator can't check it.

Fallback: a guest `socat` linking a pty at `/dev/serial0` to a virtio-serial port.

`ttyAMA0` is the guest console and stays that way.

Also needed in the guest:

- **`raspi-gpio` shim.** `GetSerial` runs `raspi-gpio set 14 a0` and `15 a0`
  and fails if they fail. A no-op script in `guest/bin`, installed over the real
  one when `TC2_TRAP=1`.
- **Mux pins** GPIO6 and GPIO12 already exist through gpio-mockup. The trap
  emulator doesn't see them; routing is assumed to be to the trap.

With these, `tc2-hat-comms` runs unpatched.

## Wire protocol

From `trap-control.go`/`trap-messenger.go` (camera) and spool-py
`src/util.py` (`RPi_UART._uart_loop`, trap):

- Line: `<id|type|payload>checksum\n`, checksum the sum of the bytes from `<` to
  `>` inclusive, mod 256, in decimal. Leading NULs are ignored.
- Requests from the camera have id > 0; responses carry the same id. Messages
  the trap sends on its own have id 0.
- Replies are `ACK`, `NACK` (payload: reason), `BAD_KEY`, or a typed reply.

| Request | Payload | Trap reply |
|---|---|---|
| `PING` | | `ACK` |
| `ENABLE` | seconds (camera sends 300) | `ACK`; enabled until now + seconds |
| `DISABLE` | | `ACK` |
| `STATUS` | | `STATUS_R`, JSON status |
| `RESTART` | | `ACK`, then the trap resets (about 1 s) |
| `READ_TIME` | | `READ_TIME_R` `{"UTC": iso, "Local": iso}` |
| `WRITE_TIME` | `YYYY-MM-DD HH:MM:SS` (UTC) | `ACK`, or `NACK` |
| `LS` | comma-separated names, or empty for all | `LS`, JSON `{name: sha256 hex[:10]}` for those that exist |
| `WRITE` | `name,["line", ...]` | `ACK`; lines appended to `name`, each with `\n` |
| `DECOMPRESS` | `src,dst` | `ACK`; `src` base64 lines → raw deflate → `dst`, `src` removed. `NACK` on failure |
| `DELETE` | name | `ACK` |
| `DELETE_TMP` | | `ACK`; removes `*.tmp` |
| `COMMIT` | | `ACK`; renames `*.tmp` to drop the suffix |
| `MV` | `src,dst` | `ACK` |
| `READ` | `name,offset,count` | `READ`, JSON lines |
| `STOP`, `RELEASE_SPOOL`, `RESET_SPOOL`, `OPEN_DOOR`/`CLOSE_DOOR`, `CAGE_*` | | `ACK` if accepted; trap goes to manual mode |
| other | | `NACK` |

Upload of a file `f` (`CopyData`): `LS f,f.ztmp,f.tmp`, skip if `f` or `f.tmp`
matches; else `DELETE f.ztmp` if present, `WRITE f.ztmp` in 500-char base64
chunks of raw-deflate data, `DECOMPRESS f.ztmp,f.tmp`, `LS f.tmp` to verify.
`COMMIT` follows once all files are written.

Sent by the trap on its own (id 0): `STATUS_CHANGED` (JSON status), `MOTION`,
`ENABLED`, `DISABLED`, `TRIGGERED`, `DRY_TRIGGERED`, `SPOOL_RESET`,
`SPOOL_RESET_FAILED`, `MAX_MOTION`, `RUNNING`, `ERROR_CODE`, `EXCEPTION`,
`RTC_BATTERY_FLAT`, `DOOR_OPENED`/`DOOR_CLOSED` (payload: door number),
`MANUAL_MODE`, `STOPPED`, `MANUAL_MODE_TIMEOUT`, `MANUAL_ERROR`, and
`CAGE_NO_ENTRY`, `CAGE_CAPTURED`, `CAGE_HOLDING`, `CAGE_RELEASING`,
`CAGE_KILLING`, `CAGE_WAITING`, `CAGE_FINISHED` (JSON payloads).

Status JSON (spool-py README): `State` (`RESETTING`, `RESET_FAILED`, `ARMED`,
`TRIGGERED`, `DRY_TRIGGERED`, `CAGE`, `MANUAL`), `Enabled`, `DisabledReason`,
`Request`, `CagePhase`, `ObservationMode`, `CameraEnabled`.

## Trap model

Two levels, behind the same socket and control API.

### A. Scripted model (`crates/tc2-emu-trap`, first)

A Rust model of the protocol and the trap's observable behaviour:

- **Framing**: parse and emit lines; bad checksums are dropped, as the trap
  does.
- **File store**: an in-memory map of name to bytes, with the commands above,
  including base64 + raw-deflate `DECOMPRESS` and sha256[:10] `LS`. Persists
  across `RESTART`, not across emulator restarts. Exposes the committed
  `config.json` and `.mpy` hashes.
- **Enable**: camera enable expiry from `ENABLE` seconds; `CameraEnabled` in
  status; `DISABLED` with "Camera stopped enabling the trap" on expiry.
- **Sequence**: `RESETTING` → `ARMED`; `MOTION` while armed and enabled →
  `TRIGGERED` (or `DRY_TRIGGERED` in observation mode) → `RESETTING` after a
  configurable delay (short by default). `STATUS_CHANGED` on each change.
- **Manual requests**: `MANUAL` state, door and spool messages, `RESTART` to
  leave it.
- **Cage**: enough of the cage phases to emit `CAGE_CAPTURED`, accept
  `CAGE_*` decisions and emit `CAGE_FINISHED`.
- **Clock**: an offset from container time, set by `WRITE_TIME`.
- **Restart**: goes silent for about a second, then `RESETTING` again.

It doesn't interpret `config.json`; the delays it would set are control API
inputs instead.

### B. Real spool-py (later, optional)

Run spool-py under MicroPython's unix port with a `machine` shim: `Pin`, `PWM`,
`ADC`, `I2C` (PCF8563 and INA219 models), `UART` (to the socket), `freq`, and
`reset` (re-exec). The rotary switch, which selects the program, and the
PIR/reed inputs come from the control API. spool-py is about 4.5k lines and
uses `_thread`, `deflate`, `hashlib` and `binascii`, all in the unix port.

It would run the `.mpy` files the camera uploads, so it tests the software
update path and the trap's real behaviour, and catches protocol drift between
tc2-hat-controller and spool-py. The MicroPython build must match the `.mpy`
bytecode version that `spool-mpy` releases are compiled for.

## Control API

`tc2-emu-trap --socket $STATE/trap.sock --listen 0.0.0.0:9001`.

- `GET /state`: status JSON as the trap would send it, camera enable expiry,
  file store (names, sizes, hashes), committed `config.json`, clock, restart
  count, and the message log (both directions, timestamped).
- `POST /motion`: PIR motion.
- `POST /cage/captured`, `POST /spool-reset-failed`, `POST /error` with
  `{"code": n}`, `POST /exception`: inject events.
- `PATCH /` with any of: `observation_mode`, `rtc_battery_flat` (sent on next
  start), `reset_delay_secs`, `silent` (unplugged: no replies), `drop_rate`,
  `corrupt_rate` (for comms retries).
- `DELETE /files`: wipe the file store, as a fresh trap.

## Classifications

`trapState` needs `org.cacophony.thermalrecorder.Tracking` signals with species
and confidences; the flat frames the emulator streams produce none. First
step: a guest tool, run by tests over SSH, that emits `Recording` and
`Tracking` signals (13 fields: clip id, track id, scores, what, confidences,
region, frame, mass, blank, tracking, last prediction frame, model id, track
start time). It must be sent from the bus name `tc2-hat-comms` matches
(check), and it relies on comms' default labels when
`ClassificationLabels` isn't served.

Later: real CPTV fixtures through the real recorder and classifier.

## Tests

In a new `crates/tc2-emu-e2e/tests/trap.rs`, with a `TrapClient` for the
control API alongside the existing hardware client:

1. **Connect**: with trap enabled on the API, the trap gets PING, the `.mpy`
   files and `config.json`, a COMMIT, a RESTART, then the time. A second
   connection with nothing changed uploads nothing and doesn't restart.
2. **Armed**: ENABLE at startup, renewed each minute; stop `tc2-hat-comms` and
   the trap's enable runs out. Armed still honours the protect list: a protect
   species → DISABLE, then ENABLE again once the protect duration has passed;
   a target species → no change, as it's already enabled.
3. **Safe**: disabled at startup; a target species above its confidence →
   ENABLE; a protect species → DISABLE, overriding the target; below the
   confidence → no change.
4. **Config sync**: changing trap settings on the API restarts comms on the
   device and changes what the trap gets (state, or `config.json`).
5. **Low power mode**: never enabled.
6. **Events**: `TRIGGERED`, `CAGE_CAPTURED` and `SPOOL_RESET_FAILED` from the
   trap reach the API as `trapTriggered` etc. without waiting for the
   scheduled upload.
7. **Management interface**: trap page actions (open door, release spool,
   restart) arrive at the trap; status changes reach `Status`.
8. **History**: `/var/lib/tc2-hat-controller/trap-history.jsonl` records sent,
   received and failed messages.
9. **Faults**: dropped and corrupted lines, a silent trap, and a trap that
   restarts mid-upload (`DELETE_TMP` on the next connect).

## Milestones

- **T1, UART**: `TC2_TRAP=1`, `pci-serial` in `entrypoint.sh`, `/dev/serial0`
  udev rule, `raspi-gpio` shim; a loopback check that bytes get through both
  ways.
- **T2, model A**: framing, file store, enable, status, restart, control API;
  test 1.
- **T3, classifications**: the signal injector; tests 2-6.
- **T4**: tests 7-9.
- **T5, optional**: spool-py under MicroPython (model B).

## Open questions

- Is `CONFIG_SERIAL_8250_PCI` in the guest kernel, and does the device appear as
  a predictable `ttyS*`?
- Which bus name and object path do `Tracking` signals need to come from for
  `tc2-hat-comms`' match rule? (It matches on interface only, so possibly any.)
- Does `tc2-hat-attiny` or anything else hold `/dev/serial0` or the mux pins in
  the image, and would that conflict?
- Should the model check `config.json` against go-config's trap config schema?
- The same UART plumbing could later carry an AT-ESL node or the modem's
  AT/GPS interface (modemd), each on its own port.
