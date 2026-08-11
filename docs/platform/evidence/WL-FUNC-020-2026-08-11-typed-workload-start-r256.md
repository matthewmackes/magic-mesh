# WL-FUNC-020 typed Android Workload start — 2026-08-11

- Scope: the production `android-lifecycle` Cloud handler now delegates governed
  outer-VM `Start` to the sole typed Workload operation lane. It requires the
  persisted desired row to be an admitted Android VM on the local placement,
  preserves requested generation and exact resource bounds, signs the complete
  Workload request, and publishes no direct libvirt effect. Rotated delivery
  capabilities preserve identical semantic fields. Stop, cancel, retry, guest
  app launch, and VDI remain explicit fail-closed boundaries.
- Production path: governed resource action → Cloud placement/capability gate →
  Android handler → `action/workload/operation` → Workload reconciler.
- Farm: BigBoy `172.20.0.130`, slot `2`. The earlier colliding `.50` run is not
  claimed as evidence.
- Focused gate:
  `workers::cloud::verbs::android_lifecycle::tests::governed_start_publishes_signed_idempotent_workload_operation`:
  PASS, 1 passed, 0 failed, 4,808 filtered out in 3m57s.
- Remaining epic boundary: typed definition from the signed Android artifact,
  Cuttlefish app launch, stop/cancel/retry, and Remote Sessions/VDI attachment.
