# WL-ARCH-009 / WL-CRIT-007 — host-state Bus recovery (r33)

Date: 2026-08-09

## Scope

`host_state` now resolves the canonical system Bus when no user root is
available and keeps the same worker alive through an unopenable startup path.
Bus open plus the fixed `action/host/<node>/verb` tail is one activation
boundary: retained host-control mutations are skipped before lifecycle
monitoring or polling begins, while durable seat snapshots remain unprimed and
fold after activation. Startup retry uses shutdown-aware exponential backoff
bounded from 10 ms through 2 s.

A failure reading the current host mirror now defers the complete action sweep
instead of substituting an empty/default mirror into power, display, or other
authorization decisions. The action cursor remains unchanged for corrected-
forward processing.

## Focused farm proof

Host: machine 193 (`172.20.0.90`)

Slot: `notify-bus-r32` (warm slot reused for this disjoint exact gate)

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::host_state::tests::late_bus_skips_retained_host_action_then_processes_forward_action \
  -- --exact --nocapture
```

Final result: `1 passed; 0 failed; 4,457 filtered out`. A prepared Bus containing
a retained host mutation replaced an unopenable startup path atomically. The
same worker skipped that retained mutation, handled one later forward request,
and shut down promptly.

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::host_state::tests::service_bus_root_falls_back_to_the_shared_system_spool \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,457 filtered out`.

The final source also passed farm single-file `rustfmt --edition 2021 --check`
and local scoped `git diff --check`.

## Artifact identity

```text
bd2368a6837a0360d698fc823723a63b048e7ae650afb27b498cc3de4595439d  crates/mesh/mackesd/src/workers/host_state.rs
```
