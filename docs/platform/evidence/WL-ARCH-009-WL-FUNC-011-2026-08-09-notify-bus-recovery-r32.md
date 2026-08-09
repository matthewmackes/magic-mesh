# WL-ARCH-009 / WL-FUNC-011 — notification Bus recovery (r32)

Date: 2026-08-09

## Scope

The `notify` worker no longer exits permanently when its Bus spool is absent or
unopenable during service startup. It resolves the canonical system spool when
no seated-user path exists and retries in the same worker with shutdown-aware
exponential backoff bounded from 10 ms through 2 s. Peer and update monitoring,
including the benign forward-lane primes required by Chat, starts only after the
Bus opens successfully.

The external Cloud notification lane is durable input to the current status
rollup. A failed read now retains both its cursor and current rollup and retries
later; it is never interpreted as an empty lane. Once activated, the worker
continues to fold queued Cloud history from its existing durable cursor.

## Focused farm proof

Host: machine 193, `172.20.0.90`

Slot: `notify-bus-r32`

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::notify::tests::late_bus_recovers_in_the_same_worker_and_primes_forward_lanes \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,447 filtered out`. The worker stayed alive while
its configured Bus path was an unopenable file, activated after that obstruction
was removed, and published exactly one prime on each forward notification lane.

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::notify::tests::service_bus_root_falls_back_to_the_shared_system_spool \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,455 filtered out` (the shared farm sync also
contained disjoint in-progress worker tests). Explicit roots remain unchanged;
an unresolved root becomes `mde_bus::SYSTEM_BUS_ROOT`.

Farm single-file formatting and scoped integrity gates passed:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/notify.rs
git diff --check -- crates/mesh/mackesd/src/workers/notify.rs \
  docs/platform/evidence/WL-ARCH-009-WL-FUNC-011-2026-08-09-notify-bus-recovery-r32.md
```

## Artifact identity

```text
b742e539c20b5cadc4821a64a59fe8df9fd446d84bb5ce3dbc1cc611c446db51  crates/mesh/mackesd/src/workers/notify.rs
```
