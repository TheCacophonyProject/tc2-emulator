# Upstream-only patches

Changes for upstream repos that the emulator doesn't build, unlike `guest/patches`.
Each is against the repo's main branch at the time.

- `tc2-hat-controller/0001-trap-settings-from-portal.patch`: every tc2-hat-comms output
  (trap-control, simple, at-esl) reads how to act on classifications from `[trap]`
  (species, default state, durations), which `[trap].enabled` turns on and off; `[comms]`
  just picks the output. Against main at fdaed59 (2026-11-12; moves the trap-duration/protect-duration check it added to `[trap]`). It needs go-config with
  `guest/patches/go-config/0003-trap-settings.patch` released, which removes the trap
  settings from `[comms]`.
- `cacophony-web/0001-trap-species-confidences.patch`: trap `target`/`protect` become maps of
  species to the confidence the device needs, from the tuned `TrapSpeciesConfidence` table
  (80 for now), and drops the unused `hasKillMechanism`. Against main at 3f40426.
- `cacophony-web/0002-thermal-recording-instant-classify.patch`: adds an optional `instantClassify` to
  `ThermalRecordingSettings` (with `useLowPowerMode` it is the medium power mode), for
  `guest/patches/go-config/0009-config-sync-instant-classify.patch` to sync. The API's schemas are
  generated with `additionalProperties: false`, so a server without this rejects the device's whole
  settings upload once the device sends `instantClassify`: **deploy it before releasing go-config 0009**.
  Against main at 3f40426 (2026-11-05).
- `saltops/0001-managementd-socket-activation.patch`: for `guest/patches/management-interface/0003-socket-activation-and-idle-exit.patch`
  (managementd started by `managementd.socket`, exiting when idle). Enables `managementd.socket`
  instead of keeping `managementd` running, and the delayed restart after an update stops
  managementd and starts the socket, as restarting managementd alone wouldn't hand it the socket.
  Against dev at 8b8efca (2026-11-17). Doesn't bump the pinned management-interface version: it
  must go in with the release that has the patch, whose package has the socket unit.
- `saltops/0002-remove-ifupdown-nfs-common-console-setup.patch`: drops the `tc2/network-manager` state
  (the `eth0` debugging config, which nobody uses) and purges `ifupdown`, `nfs-common` and
  `console-setup`, none of which a TC2 uses, and which cost ~8s of CPU at boot. NetworkManager
  does the wifi and the modem's `usb0`, so ifupdown's `networking.service` and hotplug udev rule
  do nothing useful; `nfsrahead` runs and fails for every block device; and console-setup's udev
  rules set each virtual console's font and UTF-8 mode, for an HDMI console nobody uses.
  Against dev at 8b8efca (2026-11-17).
- `saltops/0003-remove-man-defer-logrotate.patch`: on top of 0002. Purges `man-db`, `manpages`,
  `manpages-dev` and `groff-base` (nobody reads a manpage on a TC2, and man-db's dpkg trigger
  rebuilds its index on package installs such as salt's updates), replacing the state that only
  disabled `man-db.timer`. Adds a `logrotate.timer` drop-in so logrotate runs 5 minutes after
  boot and then daily, instead of straight away at boot to catch up (the timer is `Persistent`).
  Against dev at 8b8efca (2026-11-17).
- `<repo>/0001-compress-deb-with-xz.patch` for `go-config`, `event-reporter`, `device-register`,
  `management-interface` and `tc2-hat-controller`: sets `deb.compression: xz` in each repo's
  `.goreleaser.yml` `nfpms:` entry. nfpm defaults to gzip, and the packages are downloaded over the
  cellular modem. A snapshot build of event-reporter 3.14.0 went from 4.03MB (gzip) to 3.49MB
  (xz), 13% smaller; zstd:19 gave 3.66MB. `goreleaser check` passes for each. Against main at
  7e53ad0 (go-config), cd3f251 (event-reporter), f5c0d91 (device-register), f9a1a39
  (management-interface) and fdaed59 (tc2-hat-controller), 2026-11-23. tc2-agent's cargo-deb
  already uses xz.
- `tc2-agent/0001-skip-unit-until-registered.patch` and `go-config/0002-skip-unit-until-registered.patch`:
  add `ConditionPathExists=/etc/salt/minion_id` to `tc2-agent.service` and
  `cacophony-config-sync.service`, as managementd, tc2-hat-attiny and salt-minion have. Without it
  both start on an unregistered device, exit with "device is not registered" and wait to restart
  (5s and 30s) until device-register restarts them; with it, systemd skips them at boot and
  device-register's "start services skipped at boot" starts them once registered. Against main at
  aa914df (tc2-agent) and 7e53ad0 (go-config), 2026-11-24. The emulator does the same with a drop-in
  in `scripts/guest-setup.sh`, since its image uses the packages' units.
- `tc2-agent/0003-always-restart-rp2040-logs-after-programming.patch` (also `guest/patches/tc2-agent/0005`, which
  the real-device image and the emulator build apply): the agent stops `tc2-rp2040-logs.service` while it programs or
  erases the RP2040 and started it again only if it had found it running. On the first boot the agent is
  restarted mid-programming, so the next run finds it stopped and never starts it, and rp2040 logs stay
  off until the next boot. It now always starts it afterwards. Reproduced on a device by stopping the
  service, creating `/etc/cacophony/program_rp2040` and restarting tc2-agent. Against v0.12.5, 2026-11-25.
- `guest/patches/classifier-pipeline/0001-watch-config-before-trusting-the-loaded-copy.patch` (for upstream too; the
  emulator applies it to the installed PyPI package in `scripts/guest-setup.sh`): thermal-recorder-py loads
  `config.toml` at the start of `pi_classify()` but only starts watching it in `piclassify.main`, several seconds of
  imports later on a Pi Zero 2 W, so a change written in between is never seen and the old settings run until the
  next change (seen as run-classifier staying off, so the model server never started). It takes the file's mtime
  before loading and restarts if it differs once the watch is armed. Against main at 4d7575df (the same three files
  as 0.0.76), 2026-11-24. Not unit tested; `changed_since`/`config_mtime` are in `piclassifier/monitorconfig.py`.
- `tc2-hat-controller/0002-order-hat-services-on-the-i2c-dbus-name.patch`: `tc2-hat-i2c.service` becomes
  `Type=dbus` with `BusName=org.cacophony.i2c` (started once it owns the name), and `tc2-hat-rtc` and
  `tc2-hat-attiny` get `After=` and `Wants=tc2-hat-i2c.service`. Both need that name at start and
  only i2c ordered itself before them (rtc had no order against it at all, and logged "will error the
  first time"). See `docs/service-startup.md`. Against v0.36.1 (also applies to main at fdaed59). The
  matching code change, so a transient "no I2C service" isn't read as a missing ATtiny, is
  `guest/patches/tc2-hat-controller/0002-attiny-wait-for-i2c-service-before-reprogramming.patch`
  (also for upstream). Deliberately not `Type=dbus` on tc2-hat-attiny: its start can include a
  firmware update, which a start timeout would kill, and tc2-agent isn't ordered after it (it starts
  early, before multi-user.target, which attiny waits for; ordering them would delay the agent at
  every boot).
- `tc2-agent/0002-order-after-the-i2c-service.patch` (on top of 0001): `tc2-agent.service` gets
  `After=` and `Wants=tc2-hat-i2c.service`. The agent talks to the ATtiny through `org.cacophony.i2c`
  directly ("Set ready" etc., `dbus_attiny_i2c.rs`), not through tc2-hat-attiny, and exits after 10
  failed attempts 2s apart ("Failed reading ready state from attiny") if the I2C service isn't up.
  Cost: on a registered boot the agent starts after tc2-hat-i2c rather than a moment before it
  (30.4s against 31.2s on the emulator, where the agent then retried for under a second); it
  can't do anything useful sooner. tc2-agent is `DefaultDependencies=no` for an early start, and
  tc2-hat-i2c is not, so on real hardware this should be measured before releasing.
- `guest/patches/classifier-pipeline/0002-fork-services-before-starting-threads.patch` (for upstream too; on top of
  0001, applied by `scripts/guest-setup.sh`): `piclassify.main` forked the model server (and postprocess) after
  starting the snapshot thread, which logs as it starts. A fork while another thread holds the stderr buffer's lock
  leaves the child with that lock held and no owner, so the child blocks on its first log line forever: the model
  server sat started but silent for minutes and nothing could classify (seen twice in full e2e runs). Caught with
  gdb on a hung child (one thread, in `PyThread_acquire_lock` from a Python I/O write). Moves the service forks to
  just after `processor.start()`, before the snapshot and watchdog threads. 40 of 40 comms off/on cycles then came
  ready in 17-28s (unfixed: stuck at cycle 2). `watch_services` still restarts a dead service with threads running,
  where the same risk applies. Against main at 4d7575df plus 0001, 2026-11-24.
