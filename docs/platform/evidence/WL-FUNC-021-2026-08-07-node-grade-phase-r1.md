# WL-FUNC-021 — node-grade sampling phase audit (2026-08-07)

## Scope

Audited `crates/mesh/mackesd/src/workers/node_grade.rs` for synchronized
expensive sampling and repeated `system-mesh-health` publication.

Each cycle performs the resource CPU-counter interval, mesh-status and service
evidence reads, workstation audio evidence (cached separately), device
inventory read, node-row write, folded-snapshot write, and the corresponding
typed Bus publications. The worker previously performed its first sample
immediately on every host and then anchored the 10-second cadence to that
common start.

## Change

Added `initial_phase_for(hostname)` with a deterministic FNV-style hash and a
hard maximum of 1,500 ms. `NodeGradeWorker::run` waits for that phase with a
shutdown-aware `tokio::select!` before the first sample; the existing 10-second
poll interval, row/snapshot freshness windows, lifecycle merge, action
generation checks, and critical-condition edge notifications are unchanged.

Semantic publication coalescing was intentionally not applied. The published
records carry generation, observation/publication timestamps, validity, mutable
lifecycle fields, and action-generation state; suppressing equal-looking rows
or Bus messages would weaken freshness or edge semantics. The bounded phase
spreads the expensive work without changing those contracts.

## Verification

Farm host: BigBoy `172.20.0.130`, slot `node-grade-health-r1`.

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=node-grade-health-r1 \
MCNF_BUILD_SHAPE=medium \
install-helpers/xcp-build.sh cargo test -p mackesd node_grade \
  --features async-services --locked -- --nocapture
```

Result: 13 passed, 0 failed. The focused set included
`workers::node_grade::tests::initial_phase_is_bounded_and_stable_per_host`,
the existing publication-validity, lifecycle/action, conflict-file, resource,
service, and critical-condition coverage, plus the worker-role and duplicate
notification checks selected by the filter.

`git diff --check -- crates/mesh/mackesd/src/workers/node_grade.rs` passed.
The file-scoped farm `rustfmt --edition 2024 --check` probe reached the source
but reports pre-existing formatting drift elsewhere in this already-dirty
file (health import ordering and older test assertions); it was not used as a
passing gate and no unrelated formatting was rewritten.
No live-seat CPU measurement was claimed; this record is source and farm
verification only.
