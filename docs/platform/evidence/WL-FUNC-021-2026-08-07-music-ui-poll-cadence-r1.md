# WL-FUNC-021 — Music UI retained-state poll cadence (2026-08-07)

## Change

The embedded Music surface now polls the retained daemon workspace Bus record
at a bounded 500 ms cadence instead of reopening persistence and decoding JSON
on every egui frame. Daemon-authoritative surfaces schedule the next repaint at
that deadline; standalone compatibility surfaces do not acquire a daemon-only
poll loop.

## Verification

The first farm attempt exposed and was corrected for a test-fixture-only missing
`Instant` import. The corrected full library gate on farm `.90`, slot
`music-ui-workspace-poll-r2`, passed `55 passed, 0 failed`.

This is source/farm work only. Live render and installed-seat CPU proof remain
open while Dell is unreachable.
