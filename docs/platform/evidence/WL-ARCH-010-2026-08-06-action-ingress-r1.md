# WL-ARCH-010 evidence — bounded Workload action ingress (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The sole `action/workload/operation` consumer no longer calls the unbounded
retained-history query during recovery. `mde-bus::Persist::list_since_limit`
enforces the page size in SQLite before rows are decoded, and
`WorkloadComputeWorker` admits at most 64 messages per poll. The existing
exclusive ULID cursor still advances through every admitted row, including
malformed or non-target messages, so recovery makes deterministic progress
without skipping history or creating a process-local backlog proportional to
retention depth. No second worker, Bus topic, command launcher, or lifecycle
authority was introduced.

## Farm verification

All build and test work ran through explicit farm hosts and isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=bus-list-limit-r1 \
  bash install-helpers/xcp-build.sh \
  cargo test -p mde-bus persist::tests::list_since_limit_bounds_rows_and_preserves_cursor_order
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=workload-ingress-r1 \
  bash install-helpers/xcp-build.sh \
  cargo test -p mackesd --features async-services --lib \
  workers::workload_compute::tests::action_recovery_reads_a_bounded_page_and_advances_the_cursor
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=bus-persist-r2 \
  bash install-helpers/xcp-build.sh cargo test -p mde-bus persist::tests::
result: 38 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=workload-module-r2 \
  bash install-helpers/xcp-build.sh \
  cargo test -p mackesd --features async-services --lib \
  workers::workload_compute::
result: 20 passed, 0 failed
```

`rustfmt --check --edition 2021` passed for the touched `mde-bus` persistence
file on `.50`. The existing large `workload_compute.rs` module has unrelated
pre-existing rustfmt drift; its farm check reports those unrelated hunks as
well as the new test hunk. No whole-file formatter rewrite was applied because
that would overwrite unrelated in-progress work. `git diff --check` passes.

The focused regressions are:

- `list_since_limit_bounds_rows_and_preserves_cursor_order`
- `action_recovery_reads_a_bounded_page_and_advances_the_cursor`

## Remaining proof

The bounded page removes the retained-history materialization risk but does not
prove live libvirt/Quadlet execution, restart/crash recovery against real
providers, Display1/KMS attachment, caller migration, or Dell/seat-15
acceptance. Those obligations remain `Remaining` under WL-ARCH-010 and the
drain goal. Dell runtime services were not mutated or rebooted.
