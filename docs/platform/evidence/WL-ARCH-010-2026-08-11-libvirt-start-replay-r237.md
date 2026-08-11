# WL-ARCH-010 libvirt start replay recovery — 2026-08-11

- Scope: the sole Workloads libvirt actuator now treats an `already active` /
  `already running` response to a replayed `virsh start` as the committed result
  of a crash-before-journal boundary. Unrelated libvirt failures remain errors,
  and command execution remains timeout-bound.
- Farm: `172.20.0.90`, slot `1`.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::replayed_libvirt_start_accepts_already_active_backend -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,791 filtered out.
