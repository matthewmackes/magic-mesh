# WL-FUNC-021 — Music handoff state bounds (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
the daemon owner-yield integration, position-continuous seat transfer, live
DLNA control, and hardware acceptance are not yet proven.

## Invariant

The Music handoff state reader admits bounded, validated projections without
changing the daemon's single state authority. It reads at most the newest 64
valid handoff intents and newest 64 valid peer heartbeat snapshots, independent
of filesystem directory order. Oversized records, malformed JSON, unsafe peer
or intent identities, path separators, and control characters are ignored or
rejected before projection.

## Implementation

`crates/services/mde-musicd/src/state.rs` adds bounded byte reads, identity and
record validation, deterministic newest-first intent retention, newest-peer
retention with stable output ordering, and guarded intent deletion. Existing
legacy state decoding remains supported within the same byte bound; no second
handoff or playback authority was introduced.

## Hostile coverage

- `handoff_intent_reader_keeps_newest_bounded_backlog` writes 65 intents and
  proves only the newest 64 survive, with the newest winner retained.
- `peer_state_reader_keeps_newest_bounded_backlog` writes 65 peer snapshots
  and proves the oldest is excluded while the newest is retained in stable
  peer order.
- `oversized_handoff_state_records_are_ignored` proves oversized intent and
  authoritative state files do not enter the parser.

## Farm verification

- BigBoy full `cargo test -p mde-musicd -- --nocapture`: **154 passed, 0
  failed**; doctests: **0 passed, 0 failed**.
- Farm focused state module: **11 passed, 0 failed** on `.50`.
- Farm touched-file `rustfmt --edition 2021 --check`: passed on `.90`.
- Local `git diff --check`: passed.

Source SHA-256:

```text
3a878d1f0b94d8ce3deeec8c9db5e922be1e0370b611cbbd599e9002104f75f4  crates/services/mde-musicd/src/state.rs
```

## Open acceptance

The daemon still needs a farm/live proof that an admitted takeover causes the
current owner to yield, preserves queue and position for the requesting seat,
and completes through an admitted DLNA or typed seat target. This slice does
not claim those runtime effects.
