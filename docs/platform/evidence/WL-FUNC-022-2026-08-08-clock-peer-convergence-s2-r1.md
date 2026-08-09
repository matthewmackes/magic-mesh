# WL-FUNC-022 Clock peer convergence S2 — 2026-08-08

The supervised Clock worker now eagerly delivers each locally owned schedule to
its bounded approved target set and consumes only exact per-peer retained Clock
snapshots. Peer commands remain signed, target-specific, revision-bound, and
deterministically identified, so delivery retries and reordered duplicates are
idempotent. A peer may remove its own received copy without changing the source
or other targets.

Stop and Snooze acknowledgements propagate by global event identity and actor
clock. The deterministic fold makes Stop win an exact tie. A selected target can
execute while the source is absent, rejoin catches up without duplicate ringing,
and a schedule first received after its due time is recorded Missed rather than
ringing late. Unapproved origins fail before persistence or effects.

## Verification

- Farm `.196`, slot `func022-clock-peer-convergence-s2-r1`:
  `cargo test --locked -p mackesd --lib workers::clock::tests --features
  async-services -- --nocapture` passed 3/3, with 4,440 unrelated tests
  filtered.
- The three-node fixture covers source loss, target loss/rejoin, duplicate and
  reordered delivery, local removal, selected-target execution, global Stop
  convergence, and missed-late receipt.
- The local restart fixture also proves durable absolute deadlines and exact
  replay of pending queue-independent audio effects until typed receipt.
- Scoped rustfmt and diff checks passed; the authoritative Jiff 0.2.21 lockfile
  was not rewritten.

## Remaining acceptance gap

The deterministic fixture is in-process over real Bus persistence. Multi-process
BigBoy fault injection, an explicit user-facing origin blocklist control, and
live capable-peer execution/rejoin proof remain. S2 and FUNC-022 therefore stay
`Remaining`.
