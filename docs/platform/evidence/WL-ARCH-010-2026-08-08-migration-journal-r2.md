# WL-ARCH-010 durable migration command journal — 2026-08-08

Cold-migration effects still execute only through `WorkloadComputeWorker`, but
the reconciler boundary is no longer volatile. Each bounded, typed migration
command is atomically persisted as `Pending` before its actuator call, changed
to `Applied` after a terminal result, and removed only after the containing
directory is synchronized. Startup recovery replays only pending commands;
applied records are cleaned without repeating their effects. Retryable recovery
is paced at the same bounded backoff used by Workload reconciliation.

Journal admission rejects unknown fields, duplicate JSON keys, oversized domain
definitions, invalid identities, symlink/non-regular records, and more than 32
retained commands. The production libvirt adapter treats replayed shutdown,
define/start, observation, and relinquish operations idempotently while still
refusing unrelated backend errors.

## Verification

- `lint-workload-authority.sh --self-test`: passed.
- `lint-workload-authority.sh`: passed; the gate now requires the durable
  migration boundary as well as reconciler ownership.
- BigBoy `.130`, slot `arch010-migration-journal-tests-r2`:
  the first broad test-target build exhausted `/home` before execution. After
  reclaiming completed isolated slots, slot
  `arch010-migration-journal-tests-r3` ran
  `cargo test --locked -p mackesd --lib workload_compute::tests -- --nocapture`:
  31 passed, 0 failed, 4,348 filtered out.
- Focused remote rustfmt for `workload_compute.rs`: passed.
- `git diff --check`: passed.

## Source hashes

```text
9d83dfc666f9b873a04fc76186b135e2cd7b91c301bffb487228c0f338d7ae5b  crates/mesh/mackesd/src/workers/workload_compute.rs
4de755c08ec1ca42ad549b52d7337af7a0505f9f641b73b90e173ef562a5b7d4  install-helpers/lint-workload-authority.sh
394f4c87c7aca883098e9605a8458f04bb8ad472b630842ad6bc487a5e47a3dc  docs/platform/workload-authority-inventory.md
```

## Remaining acceptance gap

This closes the node-local migration-command loss window, not the entire
distributed migration protocol. Source/target event cursors and pending
commit/rollback state in `compute_migrate` remain memory-backed, and live
libvirt crash/recovery plus Dell/seat-15 attachment proof remain open. ARCH-010
therefore remains `Remaining`.
