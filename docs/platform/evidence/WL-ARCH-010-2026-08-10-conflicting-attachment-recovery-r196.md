# WL-ARCH-010 — conflicting Display1 runtime recovery

- Date: 2026-08-10
- Scope: recovery evicts a conflicting node-local Display1 runtime for the
  same Workload before installing the exact persisted attachment lease.
- Implementation: `crates/mesh/mackesd/src/workers/workload_compute.rs`
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `arch010-conflicting-attachment-recovery-r196`
- Gate:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-conflicting-attachment-recovery-r196 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::conflicting_display1_runtime_is_evicted_before_recovery_replacement -- --exact --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4730 filtered out`.
- The hostile fixture proves the replaced runtime's node-local socket is
  removed, the persisted lease is the one installed, and the attachment map
  retains exactly one runtime for the Workload.
- Live limit: this proves deterministic node-local recovery cleanup only; it
  does not prove physical guest boot, QEMU registration, KMS presentation, or
  Dell/seat-15 lifecycle recovery.
