# WL-ARCH-010 admission wiring evidence — 2026-08-06

## Scope

This checkpoint closes the implementation gap identified by the prior Quadlet
and backend-admission checkpoint: the `workload_compute` reconciler now uses
backend-specific admission and live storage observations rather than the
legacy single-pool admission call.

## Implementation

- `WorkloadStorageCapacity` is populated from separate VM and managed
  container paths.
- Active reservations are partitioned by `LibvirtVirtqemud` and
  `QuadletSystemd` before admission.
- `admit_workload_for_backend` is called at the reconciler admission boundary,
  so a container request cannot consume VM capacity or vice versa.
- The rootful Quadlet actuator and image load/check path use the managed
  `/var/lib/mde-vms/containers` graphroot created by the storage worker.
- The storage worker rejects pool/subtree symlinks, non-direct paths, and
  non-directory substitutions before creation or labeling.
- The Browser VM helper and activation contract continue to use only typed
  Workload operations; direct backend commands remain rejected.

## Validation

- BigBoy `.130`, slot `arch010-admission-focused-20260806-r4`:
  `CARGO_INCREMENTAL=0 cargo test --locked -p mackesd
  workers::workload_compute::tests::container_admission_uses_container_pool_not_vm_pool
  -- --nocapture` — passed 1/1.
- `.90`, slot `arch010-storage-focused-20260806-r4`:
  `CARGO_INCREMENTAL=0 cargo test --locked -p mackesd
  workers::storage::tests::hostile_container_storage_links_are_refused_before_creation
  -- --nocapture` — passed 1/1. Earlier broad storage attempts were stopped by
  farm ENOSPC/slot cleanup and are not counted as source failures.
- `.50`, slot `arch010-backend-contract-20260806-r1`:
  mesh workload contract tests passed 12/12, including separate backend
  reservation and admission behavior.
- Local read-only checks passed: Browser helper self-test, Browser activation
  contract, Browser package contract, shell syntax, `git diff --check`, and
  worklist self-test.

## Limitations

This is not live Dell/seat-15 proof. The strict live verifier still correctly
refuses when the daemon, role pin, and Bus are absent. Native KMS/EGL
attachment, package installation/upgrade proof, restart recovery, and live
container health remain open ARCH-010 work.

## Source identity

The source hashes below are recorded after the farm gates complete and before
the review sync:

- `crates/mesh/mackesd/src/workers/workload_compute.rs`:
  `d13a1f63279c7becfffff5a0dbddf8bf1c1370182b3139ce08ec3314a4a38e9b`
- `crates/mesh/mackesd/src/workers/storage.rs`:
  `99aebc3601ba64e5f5f4c7ad8b5afe64d56052f9c381fa306814a0ae9636d349`
- `crates/mesh/mackes-mesh-types/src/workloads.rs`:
  `ad488f7e755ed24b4b641e7e837d49608d10e9d75a90a61c2c0306d96281e6f6`
- `install-helpers/request-browser-vm-workload.sh`:
  `3bf29b308a5291216f07d0d03aadc798636eb754db70962b803ed5f5e9581f0a`
- `packaging/browser-vm/verify-activation-contract.sh`:
  `94919f649b468fe0ac01d2c73b08d525d5dbc3e800c0bc06830ef3036ccff139`
