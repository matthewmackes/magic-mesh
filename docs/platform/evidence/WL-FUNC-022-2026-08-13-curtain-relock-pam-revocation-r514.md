# WL-FUNC-022 lock-curtain PAM revocation (r514)

Date: 2026-08-13

## Result

A newer authoritative lock request now dominates an authentication attempt or
unlock animation already in progress. If PAM is running off-thread, the curtain
consumes its eventual verdict but refuses to let that stale grant lift the newly
locked seat. If a lock arrives after a grant has started the lift, the curtain
interrupts the lift and returns to the fully covering locked face. Password and
error state are cleared at the new lock boundary.

This closes a lock/PAM race where `loginctl`, idle policy, or an operator lock
could arrive after an earlier unlock attempt and still allow that older attempt
to expose the seat. The verifier remains off the render thread and is not
unsafely cancelled.

## Scope

- `crates/desktop/mde-shell-egui/src/curtain.rs`
- This evidence record

Concurrent System, navigation, worker, and hardware-probe changes were
preserved and excluded.

## Farm evidence

- `.50`, slot `func022-curtain-relock-test`:
  `cargo test -p mde-shell-egui a_new_lock_ -- --nocapture` passed 2/2; 1,594
  unrelated tests were filtered.
- `.90`, slot `func022-curtain-relock-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `.170`, slot `func022-curtain-relock-fmt`: exact-file
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/curtain.rs`
  passed.
- `git diff --check` passed.

The first test/Clippy workspaces were synced before four concurrent commits
landed and were stopped or rejected on that stale dependency graph. Both gates
were refreshed to current HEAD plus this slice and run once to completion. No
BigBoy lane was used.

## Remaining acceptance

First-release package integration remains, followed by the explicitly deferred,
non-blocking installed-seat proof for lock/PAM races, ordinary and focused-VDI
Clock actions, direct-DRM chrome, physical audio, restart/suspend, and
selected-peer convergence.
