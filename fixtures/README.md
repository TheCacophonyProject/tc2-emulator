Recordings used by the fake tc2-agent (`tc2-emu-agent`) for test recordings.
They aren't committed, since recordings from real cameras carry the location they
were made at; `make stubs` downloads them. `tc2-emu-agent` rewrites the location
in each recording it saves to a dummy one (`--latitude`/`--longitude`).

- `cat-trigger.cptv` — from tc2-firmware's `test-fixtures/` (commit pinned in the
  Makefile); a cat that triggers a recording.
