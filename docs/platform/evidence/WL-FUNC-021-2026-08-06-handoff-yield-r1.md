# WL-FUNC-021 — Music owner-yield handoff (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
position-continuous target resume, live DLNA/provider playback, and hardware
acceptance are not yet proven.

## Invariant

The daemon's existing `Engine` and state file remain the sole Music playback
authorities. After peer polling admits a targeted or general takeover intent,
the daemon selects the newest applicable intent, pauses the local engine,
persists `playing=false` with the current queue song and exact engine position,
and clears the intent only after the authoritative state write succeeds. A
daemon without an engine leaves the intent pending rather than claiming a
handoff that did not occur.

## Implementation

- `crates/services/mde-musicd/src/state.rs` now treats an unset `to_peer` as a
  general claim for the current owner, while retaining self-intent rejection
  and deterministic timestamp/intent-id selection.
- `crates/services/mde-musicd/src/bus_responder.rs` applies the admitted intent
  in the production serve loop, preserving the queue's current song and the
  engine's millisecond position in the durable paused snapshot.
- The bounded, validated handoff readers and path-safe intent deletion from the
  preceding slice remain in force; no second Bus, queue, or playback authority
  was introduced.

## Farm verification

- `.50` focused `cargo test -p mde-musicd handoff -- --nocapture`: **3 passed,
  0 failed**.
- BigBoy `.130` full `cargo test -p mde-musicd -- --nocapture`: **155 passed,
  0 failed**; doctests: **0 passed, 0 failed**.
- `.90` exact touched-file `rustfmt --edition 2021 --check`: passed.
- Local `git diff --check` for both touched Music files: passed.

Source SHA-256:

```text
d9a57d04d49997f68dcb9ebc12caf4787b8372c411b0bf6500a40b37edd20308  crates/services/mde-musicd/src/state.rs
ee4b0835ff5d15418b99f3922161cca686a224f4a9685909c19389cbbd0ca0a3  crates/services/mde-musicd/src/bus_responder.rs
```

## Open acceptance

The pure snapshot regression proves the durable song/position shape, and the
production serve-loop path is now reachable. A live engine/seat test is still
required to prove pause timing and target-side resume, and live DLNA/provider,
mpv/audio, and Dell/seat acceptance remain open. If the state write fails, the
intent is retained for retry; this evidence does not claim a rollback of the
already-issued pause.
