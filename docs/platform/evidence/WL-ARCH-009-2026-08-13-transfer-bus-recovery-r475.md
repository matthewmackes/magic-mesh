# WL-ARCH-009 transfer Bus recovery gate

Date: 2026-08-13

This slice keeps the transfers worker alive when the Bus is unavailable at
startup, deferring clipboard materializer binding while retaining the normal
materializer path when the Bus is available. The change is limited to the
transfer worker startup path; unrelated concurrent worktree edits were not
staged.

## Farm evidence

- Host `.130`, slot `arch009-transfer-compile-repair-check-20260813`:
  `cargo check -p mackesd --locked --lib` — PASS.
- Host `.90`, slot `arch009-transfer-compile-repair-clippy-20260813`:
  `cargo clippy -p mackesd --locked --lib` — PASS (existing warnings only).
- Host `.130`, slot `arch009-transfer-recovery-focused-20260813`:
  `cargo test -p mackesd --locked late_and_replaced_bus_recovers_identity_bound_forward_notifications -- --nocapture` — PASS (1/1).

The focused test initially exposed startup failure when the Bus was late; the
startup deferral was then implemented and the focused gate passed.

## Remaining acceptance

The full `mackesd` library test gate remains to be rerun after this repair.
Fleet/package/live proof is post-release and non-blocking per the current
acceptance policy. ARCH-009 remains `Remaining` for the broader ownership and
UI cutover work.
