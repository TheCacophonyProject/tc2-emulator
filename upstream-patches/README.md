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
