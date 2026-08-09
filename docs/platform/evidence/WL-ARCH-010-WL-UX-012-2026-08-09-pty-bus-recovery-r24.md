# WL-ARCH-010 / WL-UX-012 — PTY Bus startup recovery (r24)

Date: 2026-08-09

## Scope

The `pty_broker` worker no longer exits permanently when its Bus root cannot be
resolved or opened. It resolves the canonical `/run/mde-bus` fallback and retries
startup with shutdown-aware exponential backoff bounded from 10 ms through 2 s.

Activation now succeeds only after one complete `list_topics` plus tail read for
every existing `action/pty/<peer>` topic. Candidate cursors are installed only
after all reads succeed, so a partial activation cannot replay retained PTY opens.
A per-peer request topic that appears after the successful activation snapshot is
forward work: its first request drains from `None`, then its cursor advances
normally. Existing terminal open/write/resize, detach/reattach, output pump,
idle/orphan reap, and clean-shutdown semantics remain unchanged; shutdown kills
every live child without requiring the Bus.

## Focused farm proof

Host: machine 194, `172.20.0.170`

Slot: `pty-bus-recovery-r24`

The shared worktree contained an unrelated, uncommitted `chat.rs` test mismatch.
The successful gate used a detached clean `HEAD` sync in the same required slot,
then overlaid only `pty_broker.rs`; no unrelated source was changed or included.

Each exact test used this command shape from the farm slot:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::pty_broker::tests::<exact-test-name> -- --exact --nocapture
```

Affected correction rerun against the final source bytes:

- `late_bus_and_new_peer_topics_recover_without_replay_or_restart`:
  `1 passed; 0 failed; 4,440 filtered out`. The same worker survived an unresolved
  root, an open failure, and a fail-closed tail-prime failure. The startup-retained
  open produced zero effects; an existing peer's forward request, a newly created
  peer topic's first request, and its next request each spawned exactly once;
  shutdown killed all three live sessions.

Unchanged activation-boundary proofs recorded by the original r24 gate:

- `request_topic_activation_is_atomic_when_a_tail_read_fails`:
  `1 passed; 0 failed; 4,434 filtered out`. A second-topic tail failure installed
  no candidate cursor.
- `service_bus_root_falls_back_to_the_shared_system_spool`:
  `1 passed; 0 failed; 4,434 filtered out`. Explicit roots remain unchanged and an
  unresolved root becomes `mde_bus::SYSTEM_BUS_ROOT`.

Single-file farm formatting gate:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/pty_broker.rs
```

Result: passed.

Scoped local integrity gate:

```text
git diff --check -- crates/mesh/mackesd/src/workers/pty_broker.rs \
  docs/platform/evidence/WL-ARCH-010-WL-UX-012-2026-08-09-pty-bus-recovery-r24.md
```

Result: passed.

## Artifact identity

```text
e02e6db4a5dda911917985ecfbfadc76a0973b03ade82e6a9c91f51255e22039  crates/mesh/mackesd/src/workers/pty_broker.rs
```

No WORKLIST edit or commit was made.
