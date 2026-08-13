# WL-FUNC-022 lock Clock summary projection (r518)

Date: 2026-08-13

## Result

The shell now derives a bounded `LockClockSummary` from its existing
daemon-owned `ClockSnapshotV1` reader and hands that typed value to the lock
Curtain. The Curtain renders the next enabled alarm and nearest active/paused
timer without reading the Bus, storage, a clock daemon, or a second timer
store. Recurring alarms are resolved from the snapshot's civil time and IANA
zone for presentation only; no deadline is retained or advanced by the shell.

The projection is available only while the existing Clock topic was read
successfully within its two-second lease and the snapshot `node_id` matches the
caller's Clock authority. A failed/unavailable read, lease expiry, or replaced
node authority returns `None`, removing the lock summary instead of retaining
old alarm/timer truth. Lines are capped at 320 UTF-8 bytes and the Curtain can
receive at most one alarm line and one timer line.

## Farm gates

- `172.20.0.90`, slot `func022-clock-summary-test`:
  `cargo test -p mde-shell-egui lock_summary_requires_fresh_same_authority_clock_projection -- --nocapture`
  passed 1/1; 1,597 tests filtered out.
- `172.20.0.170`, slot `func022-clock-summary-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `172.20.0.130`, slot `func022-clock-summary-fmt`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/timers.rs crates/desktop/mde-shell-egui/src/main.rs crates/desktop/mde-shell-egui/src/curtain.rs`
  passed.
- `git diff --check` passed.

An earlier `.90` compile probe observed transient errors in a concurrently
edited `health_modal.rs`; it was discarded and not counted. The successful
focused run above resynced the current shared worktree after that independent
scope landed.

## Remaining acceptance

FUNC-022 coding for the typed next-alarm/active-timer Curtain handoff is
complete. Remaining epic acceptance is first-release package verification,
then the deferred non-blocking installed-seat proof for lock/unlock, alarm and
timer visibility/removal, media/volume behavior, display coverage, suspend,
and upgrade continuity.
