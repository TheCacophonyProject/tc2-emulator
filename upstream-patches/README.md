# Upstream-only patches

Changes for upstream repos that the emulator doesn't build, unlike `guest/patches`.
Each is against the repo's main branch at the time.

- `tc2-hat-controller/0001-trap-settings-from-portal.patch`: every tc2-hat-comms output
  (trap-control, simple, at-esl) reads how to act on classifications from `[trap]`
  (species, default state, durations), which `[trap].enabled` turns on and off; `[comms]`
  just picks the output. Against main at 7d07171 (v0.35.1); also applies to v0.36.1. It needs go-config with
  `guest/patches/go-config/0003-trap-settings.patch` released, which removes the trap
  settings from `[comms]`.
- `cacophony-web/0001-trap-species-confidences.patch`: trap `target`/`protect` become maps of
  species to the confidence the device needs, from the tuned `TrapSpeciesConfidence` table
  (80 for now), and drops the unused `hasKillMechanism`. Against main at 3f40426.
