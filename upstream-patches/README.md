# Upstream-only patches

Changes for upstream repos that the emulator doesn't build, unlike `guest/patches`.
Each is against the repo's main branch at the time.

- `tc2-hat-controller/0001-trap-settings-from-portal.patch`: tc2-hat-comms controls the trap
  with the `[trap]` settings from the web portal. Against main at 7d07171 (after v0.36.1).
  It needs go-config with `guest/patches/go-config/0003-trap-settings.patch` released.
- `cacophony-web/0001-trap-species-confidences.patch`: trap `target`/`protect` become maps of
  species to the confidence the device needs, from the tuned `TrapSpeciesConfidence` table
  (80 for now), and drops the unused `hasKillMechanism`. Against main at 3f40426.
