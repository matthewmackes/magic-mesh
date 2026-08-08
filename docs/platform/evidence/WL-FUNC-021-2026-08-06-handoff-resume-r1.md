# WL-FUNC-021 — Music target-side handoff resume (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
live cpal/seat proof, external-provider/DLNA acceptance, and the complete GUI
Bus-parity migration are still open.

## Invariant

The yielding daemon remains the sole writer of its authoritative paused state.
Only after that state write succeeds does it publish one bounded completion
record for the requesting peer and clear the original intent. The requesting
daemon consumes only completions addressed to its local identity, reuses the
shared queue and admitted source/cache selection, queues the finite-track seek
before the decoder's first packet, writes local ownership, and removes the
completion after one admitted engine start. No second playback or queue
authority is introduced.

## Implementation

- `crates/services/mde-musicd/src/state.rs` adds validated, newest-first,
  newest-64 bounded completion records with safe write/clear paths.
- `crates/services/mde-musicd/src/bus_responder.rs` writes completion after the
  owner-yield state, then consumes target-addressed completions in the serve
  loop. Queue identity mismatches and unavailable source/cache paths retain the
  completion instead of claiming success.
- `crates/services/mde-musicd/src/engine.rs` adds an initial-position playback
  seam. Finite sources receive the handoff position before decoding; live or
  unseekable sources fail closed to their normal position-zero behavior.

## Farm verification

- `.50` focused `cargo test -p mde-musicd handoff -- --nocapture`: **5 passed,
  0 failed**.
- BigBoy `.130` full `cargo test -p mde-musicd -- --nocapture`: **157 passed,
  0 failed**; doctests: **0 passed, 0 failed**.
- `.90` exact touched-file `rustfmt --edition 2021 --check`: passed.
- Local `git diff --check` for the three touched Music service files: passed.

Source SHA-256:

```text
8d84ed6383b9167225720493c3fa42af232e3a343c75480cc76cba9fed3762d2  crates/services/mde-musicd/src/state.rs
5bcdcdee34580f314054b05f71e7d4eaf9b50da9bd2c0f69bb3e8c846bf0536e  crates/services/mde-musicd/src/engine.rs
344a13dfe966c75ae9e021a67e9ffdee819c83b97c7364592e01088f047637c6  crates/services/mde-musicd/src/bus_responder.rs
```

## Open acceptance

The farm suite proves bounded completion persistence and the reachable daemon
resume path, but it does not prove audible output or cross-seat timing. Live
cpal playback, two-seat position continuity, DLNA/provider playback, and Dell
hardware acceptance remain required. A missing local queue song or admitted
source/cache intentionally retains the completion for review/retry.
