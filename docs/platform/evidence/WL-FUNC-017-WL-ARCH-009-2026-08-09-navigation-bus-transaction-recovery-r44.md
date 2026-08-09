# Navigation Bus and transaction recovery checkpoint (R44)

Date: 2026-08-09
Worklist: `WL-FUNC-017`, `WL-ARCH-009`
Base commit: `55df5cce39bebce729a390c5fb3a963e0e47d76f`

## Runtime semantics

`NavigationWorker` now resolves the configured/user Bus root to a concrete
path, falling back to `mde_bus::SYSTEM_BUS_ROOT`. The supervised worker retries
late or unopenable startup in the same task with shutdown-aware exponential
backoff bounded from 10 ms to 2 s. Once active, it retains a mutable `Persist`,
calls `reopen_if_index_changed()` before each complete action snapshot, and
therefore follows replacement indexes and external post-activation writers.
Runtime Bus transaction failures are deferred with the same bounded retry
rather than terminating supervision.

Every route, progress, and cancel lane is read into one sorted candidate set
before any authority mutation, provider call, state publication, or cursor
advance. A failure reading any lane rejects the complete pass. An unavailable
lane therefore cannot look empty and successful earlier lane reads cannot cause
partial effects.

Route processing preserves the deliberate `Calculating` crash journal. A fresh
request first durably reserves generation/request identity and publishes the
calculating snapshot. If that publication fails, the worker restores both its
in-memory and durable pre-action authority, leaves the route cursor unchanged,
and safely retries in the same process. A real crash with a durable calculating
journal still follows the existing restart path: generation and replay
reservation are unwound to an explicit `InterruptedByRestart` authority.

After provider completion, final authority is durably checkpointed without an
action cursor. The final snapshot is then published and only afterward is the
cursor durably committed. A failed final publication leaves the cursor pending;
the next pass republishes the durable final checkpoint without calling the
provider again. A cursor-store failure is also rolled back in memory, keeping
the action retryable. Thus no cursor acknowledges incomplete governed effects,
while completed provider effects are not repeated after final persistence.

## Focused farm verification

Requested host: machine9 `192.168.23.50`
Reachable machine9 interface: `172.20.0.50`
Slot: `navigation-bus-r44`

The requested address timed out on three SSH attempts. Machine9's interface
inventory, queried through its reachable address, reported only
`enX0 172.20.0.50/16`; `192.168.23.50` is not assigned. Verification therefore
ran on the same machine9 through `172.20.0.50` and the exact requested slot.

The following exact tests ran:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::navigation::tests::navigation_bus_root_falls_back_to_canonical_system_spool \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::navigation::tests::failed_calculating_publication_recovers_in_the_same_worker \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::navigation::tests::failed_final_publication_republishes_without_repeating_provider_effect \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::navigation::tests::incomplete_action_lane_read_defers_all_navigation_effects \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::navigation::tests::late_bus_recovers_and_observes_external_forward_write_until_shutdown \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::navigation::tests::stale_reroute_is_refused_and_restart_never_revives_calculating_route \
  -- --exact --nocapture
```

Each command passed: `1 passed; 0 failed; 4,477 filtered out`. The late-Bus test
keeps one worker alive across not-found and permission-denied opens, folds a
retained route, observes a forward route written after activation through a
separate handle, and exits promptly on shutdown. The two publication tests
prove same-worker retry with no manual authority reset and no repeated provider
call after final persistence. The lane-read test injects failure after one
successful lane and observes unchanged authority, replay set, and cursors.

Farm formatting passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/navigation.rs
```

The first compile was blocked by an unrelated concurrent
`service_aggregator/mod.rs` refactor. Only that file's ephemeral R44 farm copy,
plus unrelated dirty `clock.rs` and `dc_snap_scheduler.rs`, was restored to
`HEAD`; no unrelated local file was changed. Compilation and all exact tests
then passed.

Source SHA-256:
`4bab7df09c91739b1d4c67319fbb4c6bc583890af9d3748b8f70f5b0aa272c0e`.

## Scope

No broad suite, package build, live-seat proof, WORKLIST edit, commit, push, or
unrelated test was run. The only verification caveat is the unavailable
requested `192.168.23.50` transport; the specified physical machine and slot
were exercised through its assigned `172.20.0.50` interface.
