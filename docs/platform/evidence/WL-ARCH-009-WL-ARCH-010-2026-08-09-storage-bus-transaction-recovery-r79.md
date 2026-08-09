# WL-ARCH-009 / WL-ARCH-010 — Storage Bus transaction recovery (r79)

Date: 2026-08-09

Farm: BigBoy `172.20.0.130`, slot `storage-bus-r79`

## Production semantics

- The storage worker no longer freezes an optional Bus root at startup. Each cycle and publication phase opens a fresh transaction against the explicit test root, or the current default root followed by canonical `mde_bus::SYSTEM_BUS_ROOT`. A late Bus and a same-path `index.sqlite` replacement are therefore visible to the same worker.
- Bus activation identifies the index by device/inode and atomically tail-primes both physical and virtual transient mutation topics. Failure to open, read either tail, or verify the final identity leaves the index and both cursors inactive, so retained destructive commands cannot replay when a Bus appears later.
- Runtime reads stage the complete virtual-lane preflight and physical action batch before physical authorization or effects. Open/list/identity errors defer the whole sweep instead of masquerading as an empty lane.
- The existing storage authority is unchanged: signed mutation authorization, typed device arming, live-topology drift validation, protected-device/in-use interlocks, bounded queue execution, and UDisks2 execution remain the gates around physical effects.
- Per-operation progress and the resulting topology projection are staged as an in-memory pending commit. Strict Bus writes and identity checks must succeed before the physical cursor advances. A write fault retains the pending result and retries publication without rerunning the already-authorized storage operation; replacement during correction resets the output position and corrects the complete result into the new index.
- Heartbeat/replacement snapshots are staged before opening their publication transaction. `last_at` advances only after a successful write and stable-index check. Typed UDisks unavailability remains an honest published backend state, while Bus unavailability remains a deferred transaction.
- The run loop performs an immediate attempt and then retries on the existing bounded poll interval, with shutdown remaining selectable while the Bus is absent or unopenable.

## Focused hostile coverage

- `storage_recovers_late_and_replaced_bus_without_replaying_retained_apply`: starts with an unopenable root, activates a late Bus without replaying its retained apply, executes its first forward apply, then replaces `index.sqlite` at the same path and repeats the retained/forward assertions.
- `action_read_failure_is_effect_free_and_preserves_cursor_for_retry`: lets the sibling-lane preflight complete, fails the final physical action read, proves zero executor/cursor/pending effects, then proves corrected-forward processing.
- `publication_failure_keeps_pending_cursor_and_retries_without_repeating_apply`: fails the first required result write after one physical execution, proves the cursor remains uncommitted, then publishes the exact pending result with no second execution.
- Existing exact tests retain authorization/replay-ledger rejection, typed unavailable-state publication, and shutdown behavior.

## Verification

The farm helper explicitly selected and synchronized the requested BigBoy slot:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=storage-bus-r79 \
install-helpers/xcp-build.sh sync
# exit 0
```

The final source was run in that warmed slot with each operation-impacting filter separately and `--exact --nocapture`:

```text
cargo test -p mackesd --features async-services \
  workers::storage::tests::<test-name> -- --exact --nocapture

storage_recovers_late_and_replaced_bus_without_replaying_retained_apply ... ok
action_read_failure_is_effect_free_and_preserves_cursor_for_retry ... ok
publication_failure_keeps_pending_cursor_and_retries_without_repeating_apply ... ok
hostile_storage_applies_never_reach_the_executor ... ok
unavailable_backend_publishes_typed_state ... ok
tick_loop_exits_on_shutdown ... ok
# loop exit 0; every invocation: 1 passed, 0 failed
```

Formatting and scoped integrity:

```text
rustfmt --edition 2021 crates/mesh/mackesd/src/workers/storage.rs
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/storage.rs
# exit 0 on BigBoy

git diff --check -- crates/mesh/mackesd/src/workers/storage.rs
# exit 0 locally
```

The first exact compile found a `Persist` guard crossing an await and making the worker future non-`Send`; the final implementation uses short-lived synchronous Bus phases and the rerun compiled and passed. BigBoy free space fell below the farm helper's resync floor after the cold build, so subsequent final-source transfers reused the already-synchronized explicit slot rather than initiating another broad sync. Committed `HEAD` copies of unrelated concurrently dirty worker files were used only inside this disposable farm slot to isolate storage compilation; no local unrelated file was changed.

## Residual effect boundary

- Physical storage mutation and Bus publication cannot be one atomic transaction. The existing durable `ActionAuthorizer` replay ledger prevents an accepted signed apply from executing again, and same-process pending output corrects forward after write failure. A process death after a physical operation but before its in-memory progress/snapshot commit can lose that exact per-operation progress row; later topology publication reports authoritative current state, but it cannot reconstruct the exact interrupted result stream.
- `virtual_storage.rs` was outside this slice's permitted write scope. The outer storage loop now requires a successful strict preflight, atomically tail-primes its transient lane per index, and does not commit its outer cursor across replacement. The nested virtual worker still owns legacy internal effect/publication ordering, including best-effort publication; a fault after preflight inside that nested tick remains outside r79's corrected transaction boundary.
- Existing post-apply workload-pool layout reconciliation remains a separately logged best-effort follow-up. This slice did not weaken or broaden any destructive-action gate to make that follow-up atomic.

## Hash

```text
c9de624da6200999b7611790fd92ebee045775ea37022319e0654c174f33a56f  crates/mesh/mackesd/src/workers/storage.rs
```

No WORKLIST edit, commit, or push was performed.
