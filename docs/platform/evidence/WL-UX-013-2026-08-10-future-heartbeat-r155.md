# WL-UX-013 — future heartbeat refusal (r155)

Date: 2026-08-10

Health reconciliation now treats a heartbeat timestamp newer than the
admission clock as unreachable rather than fresh. Clock-skewed or forged
future evidence therefore cannot keep a peer green indefinitely.

## Farm proof

Build VM `.90` (`172.20.0.90`), slot `ux013-future-heartbeat-r155`:

```text
cargo test -p mackesd --lib workers::health_reconciler::tests::future_heartbeat_is_not_admitted_as_fresh_health_evidence -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 4695 filtered out
```

Live fleet clock-skew/recovery proof remains open.
