# WL-ARCH-010 — fail-closed lifecycle projection conflict (r493)

- Recorded: 2026-08-13T11:06:38Z
- Scope: `crates/mesh/mackesd/src/workers/workload_compute.rs`
- Source SHA-256: `302269be6ddd81e44917fad11bc0cda276b52bddbf6ac2b15c8615e01a5beda6`

## Result

The sole Workloads state publisher no longer selects an order-dependent row
when its durable ledger contains contradictory lifecycle records for the same
Workload generation. It now refuses the projection before a stopped, failed,
or replacement row can hide a concurrently journaled running generation.
Exact duplicate rows remain idempotent, and a unique newer generation remains
authoritative.

This closes a presentation-authority gap that capacity reconstruction already
treated conservatively: shell consumers cannot receive a lifecycle claim that
depends on ledger iteration order while admission continues reserving the
conflicting running workload.

## Farm gates

- `.130` / `arch010-projection-conflict-test-r493`: corrected focused command
  passed 1/1 (`workers::workload_compute::tests::same_generation_lifecycle_conflict_cannot_publish_an_order_dependent_projection`; 4,931 filtered).
- `.130` / `arch010-projection-conflict-clippy-r493`: `cargo clippy -p mackesd --lib --features async-services -- -D warnings` passed.
- `.130` / `arch010-projection-conflict-fmt-r493`: package-wide `cargo fmt -p mackesd -- --check` exposed pre-existing formatting drift outside this slice and in older portions of the assigned file. A follow-up rustfmt output comparison restricted to the newly added selector and regression-test blocks passed with no diff.
- Local orchestration-only `git diff --check`: passed.

The initial focused invocation used `--exact` without the Rust module prefix
and selected zero tests; it is intentionally not counted as evidence. The
corrected warmed-farm invocation above is the acceptance gate.

## Remaining epic acceptance

Repository-wide strict Clippy, package/install validation, one typed
StartAndAttach path through real libvirt/Quadlet readiness, native
KMS/Display1 attachment and recovery, and the deferred post-release live-seat
and fleet lifecycle matrix remain open.
