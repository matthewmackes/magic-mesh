# WL-ARCH-010 migration actuator authority — 2026-08-08

Cold migration requests no longer instantiate the libvirt actuator or execute
`virsh` from `compute_migrate`. They cross a bounded in-process command/reply
channel through `WorkloadMigrationClient`; only the running
`WorkloadComputeWorker` registers the executor and drains commands through its
owned `WorkloadActuator`. Constructing unrelated workers cannot replace the
global executor.

`lint-workload-authority.sh` rejects a production `Command::new("virsh")` or
`SystemWorkloadActuator` in `compute_migrate`, and requires both the client and
reconciler drain boundary. Its hostile self-test proves those regressions are
detected.

## Verification

- Local authority lint self-test and repository scan: passed.
- Host `.50`, slot `func016-s3-mesh-r1`:
  `cargo test --locked -p mackesd migration_command_executes_only_when_workload_reconciler_drains -- --nocapture`
  — 1 passed, 0 failed.
- The focused test proves no actuator call occurs before the Workload
  reconciler drains the request, then returns the exact actuator result.

## Source hashes

```text
2790a0b917649fa10005791875c0a28835b677cd887659a0d0fc2831451099dc  crates/mesh/mackesd/src/workers/compute_migrate.rs
8462cd668c87d790416673ef49455f01344540fe9e24af675b7e56e406d5239d  crates/mesh/mackesd/src/workers/workload_compute.rs
53ef2b3b4df631efa73ad9361c2b03245cbce1638c3238bb04548e954446363c  install-helpers/lint-workload-authority.sh
d715ad9ae79f3183071fb1bb3ef2b9cacacaf84fbe13c8aa6ab9a75d996f533f  docs/platform/workload-authority-inventory.md
```

## Remaining acceptance gap

The bounded migration queue is in-process and not journaled. A daemon crash can
lose an accepted-but-undrained request, so restart-safe migration recovery and
live libvirt migration proof remain open; this checkpoint does not close
ARCH-010.
