# WL-ARCH-010 / WL-CRIT-007 — session-roaming Bus recovery (r21)

Date: 2026-08-09

Base commit: `050991328807db6b733f8a0a3e0d8f77c15f434c`

Production source: `crates/mesh/mackesd/src/workers/session_roaming.rs`

Source SHA-256:
`f05fa8732b258f121a5d958648258d4b518883f3e026b61ed1e2e4001e42f509`

## Correction

`SessionRoamingWorker` no longer exits successfully when Bus resolution or
`Persist::open` is temporarily unavailable. Explicit Bus roots remain
authoritative; the normal mde-bus data-root resolver is honored otherwise, and
a system service without a user root selects the documented
`mde_bus::SYSTEM_BUS_ROOT` spool. Startup retries at the configured poll cadence
clamped to 10 ms–2 s, with shutdown interrupting every wait.

The worker does not preload layouts, read session state, elect an actuator, or
apply roaming effects before a Bus open succeeds. Its full-log cursor remains
`None` across startup recovery, so an authorized policy queued during the outage
is folded after recovery rather than skipped. The existing exact-body
authorization and replay ledger remain unchanged.

Bus reads now have a strict production result. A failed `list_since` defers the
entire convergence tick instead of masquerading as an empty desired-state read.
The existing fold and cursor are retained, so unreadable Bus state cannot delete
or repoint a session, erase a valid policy, or cause a privileged roaming effect
to replay.

## Focused farm proof

Host: BigBoy (`172.20.0.130`)

Slot: `session-roaming-bus-r21`

```text
cargo test -p mackesd --features async-services --lib \
  workers::session_roaming::tests::default_bus_root_uses_the_shared_mde_bus_resolver \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,427 filtered out`. Explicit-root preservation and
the system Bus fallback are exact assertions.

```text
cargo test -p mackesd --features async-services --lib \
  workers::session_roaming::tests::bus_absence_wait_is_alive_and_shutdown_prompt \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,427 filtered out`. A missing resolved root leaves
the worker alive, and shutdown interrupts the bounded retry wait.

```text
cargo test -p mackesd --features async-services --lib \
  workers::session_roaming::tests::bus_open_retry_preserves_state_then_processes_queued_policy_once \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,427 filtered out`. One worker survives unresolved
root and injected open failure states without mutating a disconnected live
session, then folds a queued authorized Shutdown policy and applies its removal
exactly once after recovery.

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/session_roaming.rs
```

Result: passed on BigBoy after the final source sync. Local scoped
`git diff --check` also passed.

## Scope

No broad suite, package build, installed-seat test, or unrelated test was run.
This checkpoint proves only system Bus fallback, bounded startup recovery,
shutdown liveness, strict read-failure deferral, queued-action delivery, and
single application of the recovered destructive policy.
