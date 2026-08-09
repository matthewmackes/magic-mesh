# WL-FUNC-022 Clock contracts S1 — 2026-08-08

The shared mesh contract now defines closed, bounded Clock commands, snapshots,
schedules, occurrences, per-target state, acknowledgements, stopwatches,
settings, and stable audio references. Commands are revision- and time-bound,
signed with Ed25519, and admitted only against an explicitly trusted signer.
Raw audio URLs and malformed IANA-zone identities fail closed.

Canonical topics are `action/clock/command/<target-node>`,
`state/clock/<node>`, `event/notify/clock/<node>`, and
`reply/<request-id>`. Wire caps are 64 KiB per command and 256 KiB per
snapshot; collection and field caps cover schedules, occurrences, targets,
mirrors, laps, IDs, labels, zones, audio identities, and one-year timer and
stopwatch bounds. The command age and TTL are five minutes with 30 seconds of
future skew.

## Verification

- `.196`, slot `func022-clock-contracts-s1-r2`: focused Clock contract tests
  passed 5/5.
- The same farm slot passed the complete `mackes-mesh-types` suite 473/473 and
  its documentation tests.
- Hostile coverage includes duplicate/unknown keys, schema skew, collection
  caps, malformed IDs/times/zones, stale/future commands, revisions, raw URLs,
  untrusted signers, and signature tampering.
- Scoped rustfmt and `git diff --check` passed.
- No operational tests were removed.

## Remaining acceptance gap

The daemon scheduler must use system tzdata to resolve civil-time DST gaps and
folds; this contract slice injects zone admission but does not claim occurrence
resolution. Persistence, deadline recovery, peer receipts, block/opt-out,
missed-late handling, and convergent Snooze/Stop remain in S2, so FUNC-022 stays
`Remaining`.
