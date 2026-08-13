# WL-FUNC-022 — pre-Bus deadline recovery (r505)

Date: 2026-08-13

## Result

The Clock worker now advances elapsed alarm, timer, and scheduled-snooze
deadlines while loading durable SQLite authority, before the transient Bus is
available. The recovered snapshot and its Music audio outbox transitions commit
atomically. A later Bus generation publishes the corrected snapshot and drains
the retained outbox; restart no longer leaves an overdue timer falsely Running
while the spool is unavailable.

The recovery transaction also reconstructs deterministic Start rows only for
occurrences that remain Ringing after deadline evaluation and de-duplicates a
newly due occurrence's Start identity.

## Farm gates

- `.50`, slot `func022-clock-rustfmt`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/clock.rs` — passed.
- `.90`, slot `func022-clock-test`:
  `cargo test -p mackesd restart_persists_elapsed_timer_before_bus_recovery -- --nocapture`
  — passed 1/1 with 4,951 filtered tests.
- `.170`, slot `func022-clock-clippy`:
  `cargo clippy -p mackesd --all-targets -- -D warnings` — passed.

The focused regression makes the Bus root intentionally unusable, restarts from
persisted authority at the elapsed deadline, and proves the timer is durably
Expired, its occurrence is Ringing, and exactly one audio request is retained.

## Remaining acceptance

WL-FUNC-022 still requires first-release package integration and the deferred
post-release installed-seat proof for reboot/suspend, selected-peer execution,
global Snooze/Stop, governed physical audio, direct-DRM Clock surfaces, and
fresh-install/non-importing upgrade behavior.
