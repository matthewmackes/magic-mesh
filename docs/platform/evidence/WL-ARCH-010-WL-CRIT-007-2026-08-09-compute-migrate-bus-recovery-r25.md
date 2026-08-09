# WL-ARCH-010 / WL-CRIT-007 — compute migration Bus recovery (2026-08-09, r25)

## Scope

Corrected the P0 startup race in
`crates/mesh/mackesd/src/workers/compute_migrate.rs`. The worker no longer
permanently exits when the Bus root is unresolved or `Persist::open` fails.
Startup now resolves the canonical system spool fallback, retries with a
shutdown-aware interval clamped to 10 ms–2 s, and activates without requiring a
daemon restart after the Bus appears.

## Durability decision

Migration lanes fold retained messages from the durable migration-ledger
cursors; they deliberately do **not** prime at the Bus tail. A migration request,
ready event, committed acknowledgement, or failed acknowledgement queued during
an outage is unfinished distributed state-machine input. Tail-skipping it could
strand a prepared migration, lose a commit receipt and roll back a VM already
running on the target, or lose an operator request.

Exactly-once safety remains ledger-based:

- every source, target, committed, and failed cursor is checkpointed in the
  root-only migration ledger;
- relevant work is durably recorded as prepared before authorization/effect;
- recovery re-authorizes only owned prepared jobs and accepts only the specific
  already-consumed capability associated with that durable job;
- committed acknowledgement state is checkpointed as `Relinquish` before the
  destructive source-definition removal;
- failed/timeout state is checkpointed as `Rollback` and retained for bounded
  retry until the source is runnable again.

The run loop now completes reads of all four Bus lanes before any source
migration, target apply, commit relinquish, timeout rollback, or terminal retry.
A read error is an explicit failed sweep: all migration effects are deferred.
Any admissions completed before a later lane fails are safe because their jobs
and cursors were already durably checkpointed and are resumed, not duplicated,
on the next successful sweep.

## BigBoy exact verification

Host: `172.20.0.130` (BigBoy)
Slot: `compute-migrate-bus-r25`

Command shape used for each test:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=compute-migrate-bus-r25 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services \
  --lib workers::compute_migrate::tests::<TEST> -- --exact --nocapture
```

Exact results:

- `compute_migrate_bus_root_preserves_override_and_has_system_fallback` —
  `1 passed; 0 failed; 4,441 filtered out`.
- `unavailable_bus_retries_until_shutdown_without_touching_migration_state` —
  `1 passed; 0 failed; 4,441 filtered out`.
- `bus_read_error_is_failure_and_cannot_trigger_pending_rollback` —
  `1 passed; 0 failed; 4,441 filtered out`.
- `late_bus_folds_queued_migration_once_and_preserves_pending_commit` —
  `1 passed; 0 failed; 4,441 filtered out`.

After the final inode-reopen safeguard was added and the source was formatted,
the last test above was rerun from the final source and passed again:
`1 passed; 0 failed; 4,442 filtered out`. The filtered count changed because
another disjoint worker test entered the shared worktree; the exact selected
test and its result were unchanged.

The final single-file farm formatting gate passed:

```text
ssh mm@172.20.0.130 \
  'cd /home/mm/magic-mesh-farm-compute-migrate-bus-r25 && \
   rustfmt --edition 2021 --check \
   crates/mesh/mackesd/src/workers/compute_migrate.rs'
```

Local scoped `git diff --check` also passed. The exact Cargo invocations emitted
existing workspace warnings outside this owned source; no compute-migrate test
or compilation failure occurred.

## Source identity

```text
d45298d0bcbd3848c4c378c6650eb94d947820922e04ea0c7e0d68a4716190e0  crates/mesh/mackesd/src/workers/compute_migrate.rs
```
