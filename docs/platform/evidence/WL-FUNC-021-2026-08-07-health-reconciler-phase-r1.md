# WL-FUNC-021 — health-reconciler phase audit (2026-08-07)

`HealthReconcilerWorker` performed a SQLite/etcd/filesystem liveness pass on a
shared five-second boundary across seats. It now preserves the existing
no-immediate-pass behavior while applying a deterministic node-id phase capped
at 1,500 ms before the first pass. The first pass still occurs no later than
the normal five-second cadence, so the documented heartbeat-plus-reconcile
health transition bound is unchanged. Shutdown remains honored during the
phase wait.

Farm `.50`, slot `health-reconciler-phase-r1`:

```text
cargo test -p mackesd health_reconciler --features async-services --locked -- --nocapture
test result: ok. 13 passed; 0 failed; 4389 filtered out
```

The focused set includes the new stable/bounded/deadline test plus existing
heartbeat, etcd liveness, peer-version mirroring, signal, quiet-tick, and
worker-name coverage. This is source/farm evidence; live-seat CPU sampling
remains open while Dell is unreachable.
