# WL-UX-012 — taskbar session-owner generation binding (r550)

Date: 2026-08-13

## Production result

The shell session rail now has one explicit taskbar projection owner. The owner
is the exact client peer most recently projected by the taskbar or Sessions
surface, and every owner replacement advances a monotonic local generation.
Taskbar focus intents retain that owner identity/generation and the exact local
session incarnation that produced the row.

Consequently:

- a session ID belonging to another seat cannot be focused;
- changing the projected seat revokes all queued taskbar focus authority;
- disconnect, non-focusable App VM state, close, or same-ID replacement revokes
  the old intent;
- a close/reopen cannot transfer an old click to the replacement incarnation;
- consuming an intent first polls the lifecycle log, so a revocation arriving
  after the click but before dispatch still wins; and
- each accepted intent remains one-shot.

No broker lifecycle mutation, display fallback, duplicate taskbar owner, or
second connection path was added.

## Changed production scope

- `crates/desktop/mde-shell-egui/src/session_rail.rs`

## Hostile regression

`session_rail::tests::taskbar_focus_is_revoked_across_seat_switch_and_session_replacement`
exercises a foreign-seat ID attempt, a queued action crossing a seat-owner
switch, a disconnect racing dispatch, and a close/reopen reusing the same
public ID. Only a newly selected row owned by the current seat and exact new
incarnation may produce one focus intent.

## Gates

All heavy commands ran on the farm with explicit host and slot routing.

- `.170` slot 1 — focused module-qualified regression: source compilation
  reached `mde-shell-egui`, then stopped before test execution on the unrelated
  concurrent `main.rs:8121` initializer missing `FrontDoorPeerAppTarget::source_revision`.
  This is not recorded as a pass.
- `.170` slot 2 — `cargo build -p mde-shell-egui --all-targets`: reached the
  shell and stopped on the same unrelated `main.rs:8121` test initializer. This
  is not recorded as a pass.
- `.170` slot 1 — strict all-target/all-feature Clippy: the changed module
  compiled without a diagnostic, then the crate stopped on the same unrelated
  `main.rs:8121` error and pre-existing `communications/mod.rs:608`
  `clippy::while_let_loop`. This is not recorded as a pass.
- `.170` slot 2 — package Rustfmt check: no `session_rail.rs` delta; it remained
  red only for concurrent `front_door.rs` and `main.rs` formatting drift.
- Local scoped `git diff --check`: passed.

The gate failures were not corrected because those paths were explicitly
outside this slice and concurrently owned. No gate was rerun.

## Residual acceptance

- Execute the named hostile regression with nonzero discovery after the
  concurrent shell compile break is reconciled.
- Complete the separate multi-display ownership and lock-generation authority
  slices.
- Include the shell in the first full signed release.
- Perform deferred, non-blocking post-release physical lock, display
  reconfiguration, and session-switch acceptance.
