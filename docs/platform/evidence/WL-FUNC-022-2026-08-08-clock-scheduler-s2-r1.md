# WL-FUNC-022 local Clock scheduler S2 — 2026-08-08

The grouped mackesd Clock worker is now reachable from its worker role and owns
the local node's durable schedule authority. It admits exact-signer commands,
persists snapshots, revisions, request replay, and the Bus cursor through the
sole SQLite writer before publication, advances absolute deadlines without the
GUI, and restores elapsed timers after restart. A first-received-late timer is
recorded Missed rather than ringing late.

Persisted snapshots have a separately bounded recovery admission path so an
elapsed deadline can be loaded exactly once and transitioned by the scheduler;
live command/state admission remains strict. Semantic/authentication refusals
advance the durable cursor without mutating or republishing a different payload
under the same revision. Concurrent-writer reloads re-admit the durable snapshot
and verify node/revision identity before adoption.

The trusted Ed25519 key is read through a bounded `O_NOFOLLOW` regular-file
handle and must be root-owned and not group/world writable. Missing or unsafe
trust leaves the worker fail-closed.

## Verification

- `.50`, slot `func022-clock-s2-local-r1`: focused mackesd Clock worker tests
  passed 2/2 (4,426 unrelated tests filtered).
- Focused shared Clock contract/recovery tests passed 5/5 (475 unrelated tests
  filtered).
- Clock spawn reachability passed 1/1.
- Scoped `git diff --check` passed.
- No operational tests were removed.

## Remaining acceptance gap

Weekday civil-time recurrence, selected-peer delivery/receipt convergence,
distributed Snooze/Stop, Clock audio, UI cutover, packaging, and live suspend/
reboot proof remain. This is a local scheduler slice, so FUNC-022 stays
`Remaining`.
