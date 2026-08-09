# WL-ARCH-009 / WL-ARCH-010 — Compute migration Bus transaction recovery (r84)

Date: 2026-08-09

Farm: BigBoy `172.20.0.130`, slot `compute-migrate-bus-r84`

## Production correction

- Every read and publication pass now resolves the explicit/current Bus root with canonical `mde_bus::SYSTEM_BUS_ROOT` fallback, opens a fresh `Persist`, and binds that connection to the path's device/inode identity. An index replacement between metadata inspection and connection open, during a read sweep, or after a write is an explicit transaction failure rather than a stale success.
- The same worker retries absent/unopenable Bus state on the existing shutdown-aware bounded cadence. It no longer retains one SQLite connection or ignores `reopen_if_index_changed` failure.
- A sweep stages all four bounded lanes before any ledger admission or migration effect: source actions, target-ready actions, committed replies, and failed replies. A lane read, capacity, or final identity failure has zero cursor, authorization-ledger, migration-ledger, timeout, rollback, relinquish, or backend effects.
- Source migration and target-ready lanes are transient privileged mutation lanes. First activation and every new index atomically tail-prime both lanes, so retained commands do not execute. Already-admitted host-ledger jobs remain durable across worker restart. The committed/failed lanes are durable replies and fold from the beginning of a replacement index so outstanding migration transactions can converge.
- Source, target, relinquish, and rollback backend calls are durably claimed before execution. Restart recovery never repeats an interrupted claim: source and target claims produce a signed typed indeterminate failure outbox; interrupted terminal relinquish/rollback claims remain explicitly `Indeterminate` in the trusted local ledger. A relinquish/rollback adapter error returned after the claim, or a blocking-task join failure, is equally ambiguous: it now durably transitions to `PendingPhase::Indeterminate { operation, reason }` instead of restoring the pre-claim retryable phase.
- Ready, committed, and failed replies are signed once and their exact serialized bodies are persisted before publication. Bus open/write/identity failure retains the outbox. Publication success is followed by ledger cleanup; a crash in that gap republishes the identical capability envelope, whose replay identity prevents a second migration effect.
- Existing authority and bounds remain in force: exact-body capability checks, single-use authorization ledger, Nebula source/target binding, managed-disk path validation, 128-job/four-lane sweep bounds, 8 MiB ledger limit, root directory mode `0700`, file mode `0600`, no-follow regular-file checks, duplicate-key rejection, atomic rename, file sync, and directory sync.

## Focused hostile coverage

- `complete_read_failure_is_effect_free_then_corrects_forward`: injects failure in the final failed-reply lane after the other three reads succeed, proves zero cursor/job/backend effects, then executes the first forward action once.
- `same_path_replacement_tail_skips_retained_and_runs_forward_once`: replaces `index.sqlite` at the same path, proves the replacement's retained migration is skipped, and executes its first forward migration once.
- `open_rejects_connection_path_identity_race_without_activation`: swaps the index after connection open but before path verification and proves the stale connection/path pair is rejected.
- `late_bus_tail_activates_then_executes_first_forward_migration_once`: keeps one worker alive through late Bus availability, skips the retained command, executes the post-activation command once, and shuts down promptly.
- `durable_exact_outbox_recovers_after_write_failure_without_repeating_effect`: fails reply publication after the backend sequence, restarts from the ledger, publishes the byte-identical retained reply, and proves no backend call repeats.
- `recovered_effect_claims_publish_indeterminate_without_repeating_backend_calls`: recovers interrupted target and terminal claims, emits the typed target indeterminate result, retains terminal indeterminate truth, and makes zero backend calls.
- `relinquish_returned_error_after_claim_is_indeterminate_and_never_retried`: injects an adapter error after the durable relinquish claim, proves exactly one undefine attempt, reopens the ledger, and proves a second cycle does not retry.
- `rollback_returned_error_after_claim_is_indeterminate_and_never_retried`: injects an adapter error after the durable rollback claim, proves exactly one rollback attempt, reopens the ledger, and proves a second cycle does not retry.
- Existing shutdown and hostile ledger ownership/symlink/duplicate-key/size tests were retained as exact gates.

## Verification

The farm topology probe reported 5/5 nodes up and the helper explicitly synchronized the requested BigBoy slot:

```text
install-helpers/farm-topology.sh table
# 5/5 nodes up; 2/10 heavy slots active at the probe

MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=compute-migrate-bus-r84 \
install-helpers/xcp-build.sh sync
# exit 0
```

Final-source compile and exact hostile tests:

```text
cargo test -p mackesd --lib --features async-services \
  workers::compute_migrate::tests::recovered_effect_claims_publish_indeterminate_without_repeating_backend_calls \
  -- --exact --nocapture
# 1 passed; 0 failed; 4595 filtered out

target/debug/deps/mackesd_core-7b2dac935c32c5ff \
  workers::compute_migrate::tests::<test-name> --exact --nocapture

complete_read_failure_is_effect_free_then_corrects_forward ... ok
same_path_replacement_tail_skips_retained_and_runs_forward_once ... ok
durable_exact_outbox_recovers_after_write_failure_without_repeating_effect ... ok
recovered_effect_claims_publish_indeterminate_without_repeating_backend_calls ... ok
open_rejects_connection_path_identity_race_without_activation ... ok
late_bus_tail_activates_then_executes_first_forward_migration_once ... ok
unavailable_bus_retries_until_shutdown_without_touching_migration_state ... ok
migration_ledger_rejects_symlink_duplicate_keys_and_oversize_state ... ok
# each exact invocation: 1 passed, 0 failed; loop exit 0
```

Review-correction gates on the same synchronized slot:

```text
cargo test -p mackesd --lib --features async-services \
  workers::compute_migrate::tests::relinquish_returned_error_after_claim_is_indeterminate_and_never_retried \
  -- --exact --nocapture
# 1 passed; 0 failed; 4606 filtered out

cargo test -p mackesd --lib --features async-services \
  workers::compute_migrate::tests::rollback_returned_error_after_claim_is_indeterminate_and_never_retried \
  -- --exact --nocapture
# 1 passed; 0 failed; 4606 filtered out

target/debug/deps/mackesd_core-7b2dac935c32c5ff \
  workers::compute_migrate::tests::recovered_effect_claims_publish_indeterminate_without_repeating_backend_calls \
  --exact --nocapture
# 1 passed; 0 failed; 4606 filtered out
```

Farm formatting and scoped integrity:

```text
rustfmt --edition 2021 crates/mesh/mackesd/src/workers/compute_migrate.rs
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/compute_migrate.rs
# exit 0 on BigBoy

git diff --check -- crates/mesh/mackesd/src/workers/compute_migrate.rs
# exit 0 locally
```

The initial cold exact compile encountered three unrelated `unused_must_use` errors in concurrently modified `notify.rs`. The disposable r84 slot overlaid only the committed `HEAD` version of that unrelated file; no local concurrent file was edited or reverted. The final source then compiled and all exact r84 tests passed. Existing crate-wide warnings were outside this file's scope.

## Residual effect boundary

- A backend effect and its ledger outcome cannot be one atomic filesystem transaction. Claim-before-effect closes repeated execution: process death, an untyped adapter error, or a task join failure after that claim yields an honest indeterminate result rather than guessing success or rerunning a privileged operation. For source migration this can require operator reconciliation of a stopped source or partially transferred disk.
- The protocol has typed ready/committed/failed Bus events, but no separate wire result for post-commit source relinquish or rollback. Interrupted terminal claims therefore remain durable local `Indeterminate` ledger state and do not silently retry; exposing a new remote terminal-result schema is outside this file-only slice.
- Bus append and outbox cleanup are not atomic. Corrected-forward recovery may append the same exact signed body again after a crash, but it cannot mint a fresh replay authority and the single-use capability ledger prevents repeated backend effects.
- Tests used deterministic fake migration authority and Bus replacement fixtures. No live multi-gigabyte rsync, libvirt guest migration, or inter-peer federation proof was claimed.

## Hash

```text
eef57cc176cd03dc3e8e8bed30b30ac51606b2825038ac32e62f13c4c6e2bb9f  crates/mesh/mackesd/src/workers/compute_migrate.rs
```

No WORKLIST edit, commit, or push was performed.
