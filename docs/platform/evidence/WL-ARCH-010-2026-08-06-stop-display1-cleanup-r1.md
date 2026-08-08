# WL-ARCH-010 — terminal Stop releases Display1 immediately (2026-08-06)

Status: implementation slice complete; WL-ARCH-010 remains `Remaining` because
live QEMU Display1/KMS, adapter recovery, caller migration, and Dell/seat-15
acceptance are still open.

## Invariant

The sole typed Workload actuator owns the node-local Display1 runtime. A normal
terminal Stop must release that runtime at completion; lease expiry remains a
crash/restart safety net rather than the normal Stop cleanup mechanism.

## Implementation

- `crates/mesh/mackesd/src/workers/workload_compute.rs` adds the production
  `SystemWorkloadActuator::stopped_outcome` path. It removes the workload's
  attachment from the actuator map and returns the existing typed terminal
  projection (`Completed`, `Stopped`, `Unavailable`) with no attachment.
- The stopped/non-running observation branch uses this helper, so dropping the
  final runtime closes the listener and removes the node-local Unix socket as
  part of the existing runtime `Drop` path.
- The focused regression creates an actual Display1 attachment, invokes the
  production Stop outcome path, and proves the socket and attachment are gone.
  This keeps cleanup inside the Workload authority and does not create a
  second journal, worker, or presentation authority.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-stop-cleanup-focused-r4 \
  install-helpers/xcp-build.sh cargo test -p mackesd --features async-services \
  --lib workers::workload_compute::tests::stopped_workload_releases_display1_runtime_immediately \
  -- --nocapture
```

- `.50` focused regression: **1 passed, 0 failed**; 4,407 tests filtered out.
- BigBoy `.130` full `workers::workload_compute` module: **22 passed, 0
  failed**; 4,386 tests filtered out.
- The farm emitted the crate's existing warning set; no failure was caused by
  this slice. The local host has no Rust/rustfmt installation, so compilation
  and tests ran on the farm.

## Source hash

```text
0f4d5f2965c0d8b901681028107d4227f9cde5a3d23969e73734a0f56c2af9f1  crates/mesh/mackesd/src/workers/workload_compute.rs
```

## Open acceptance

This proves fixture-backed terminal cleanup, not live QEMU Display1/KMS scanout
or Dell acceptance. Live adapter recovery, libvirt/Quadlet recovery, caller
migration, crash/restart proof, seat-15 acceptance, and the remaining Workload
adapters stay open under WL-ARCH-010.
