# WL-ARCH-010 — expired Display1 runtime reaping (2026-08-06)

## Goal

Close the node-local Display1 resource lifecycle gap where an expired lease was
removed from the published Workload projection but the actuator still retained
the listener thread and Unix socket.

## Implementation

- `crates/mesh/mackesd/src/workers/workload_compute.rs`
  - Added the optional `WorkloadActuator::reap_expired` lifecycle hook so
    actuators without ephemeral resources remain no-op compatible.
  - `SystemWorkloadActuator` now removes expired Display1 runtimes from its
    attachment map. Dropping the final runtime shuts down the listener and
    removes its node-local socket through the existing `Drop` path.
  - `WorkloadComputeWorker::tick_once` reaps after in-flight recovery and
    before projection, allowing a recovered in-flight lease to be refreshed
    before stale resources are removed.
  - Added a hostile regression proving an expired runtime is removed from the
    actuator map and its Unix socket disappears.

This preserves the single typed Workload actuator boundary. It does not add a
second cleanup authority or mutate the durable operation journal merely because
an ephemeral attachment lease expired.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-display1-reap-focused-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --features async-services \
  --lib workers::workload_compute::tests::expired_display1_runtime_is_reaped_and_socket_is_removed \
  -- --nocapture
```

- `.50` focused regression: **1 passed, 0 failed**.
- BigBoy `.130` full `workers::workload_compute` module: **21 passed, 0
  failed**, with 4,386 tests filtered out.
- `.90` `cargo fmt -p mackesd -- --check`: reports pre-existing formatting
  drift in unrelated dirty sections, including `display1_broker.rs` and
  existing portions of `workload_compute.rs`; no whole-file rewrite was made.
- The local host has no Rust/rustfmt installation; heavy compile/test authority
  therefore remained on the farm. No new warning was emitted for this slice.

## Source hash

```text
b17c09e005710c38cd82b887caccf644aebf566b3368db2f0c8ab93ef5a4c8db  crates/mesh/mackesd/src/workers/workload_compute.rs
```

## Remaining authority proof

This closes only expired node-local Display1 runtime cleanup. Live QEMU
Display1/KMS scanout, libvirt/Quadlet recovery, caller migration, crash
recovery, Dell/seat-15 acceptance, and the remaining Workload adapters remain
open under `WL-ARCH-010`; the epic stays `Remaining`.
