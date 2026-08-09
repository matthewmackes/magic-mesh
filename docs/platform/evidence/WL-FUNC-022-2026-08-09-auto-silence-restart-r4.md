# WL-FUNC-022 alarm auto-silence and restart recovery — 2026-08-09

The Clock worker previously validated and persisted the configured
`auto_silence_minutes` value but never enforced it. An unattended alarm could
therefore remain `Ringing` indefinitely, including after daemon restart.

`crates/mesh/mackesd/src/workers/clock.rs` now transitions elapsed ringing
alarms to durable `Missed` state at the configured deadline and records every
ringing target as missed. A ringing-to-missed transition also writes a stable,
occurrence-bound Clock-audio Stop request in the same authority commit, so
restart recovery cannot retain stale alarm audio. Timers remain outside this
policy because their required recovery behavior is to alert after elapsed wall
time.

Farm machine 193 (`172.20.0.90`), slot `func022-r4-20260809`:

- Exact-file `rustfmt --check`: passed.
- Hostile restart/elapsed-alarm regression: 1 passed, 0 failed.
- Complete Clock worker suite: 7 passed, 0 failed.
- Scoped `git diff --check`: passed.
- Corrected source SHA-256:
  `4e5ee428fcda68a512ef90f0bd6621a9652bfcb1eadb3ddbb7cff97be7ca9a0f`.

The first `--locked` farm invocation stopped before compilation because the
branch lockfile was already out of sync with its manifests. The same farm slot
ran the focused gates without `--locked`; the local lockfile was not changed.

Remaining live limitation: this fixture proves durable daemon authority and the
typed audio-stop effect, not audible PipeWire shutdown after a physical daemon
restart or multi-seat unattended-alarm convergence.
