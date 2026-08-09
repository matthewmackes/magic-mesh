# WL-ARCH-009 dc-snap scheduler Bus recovery r42 — 2026-08-09

## Scope and result

`dc_snap_scheduler` now keeps the same supervised worker alive while an initially
missing or unopenable Bus recovers, using shutdown-aware exponential retry from
10 ms through 2 s and the canonical system Bus root when ordinary resolution is
absent. Each sweep stages a complete durable fold of every schedule and each
corresponding run-history lane before any snapshot, prune, alert, or publication
effect. A failed enumeration or lane read defers the complete sweep.

Durable configuration and history are replayed from the beginning on every
sweep, so retained authority is not tail-primed away and newly appearing schedule
topics are discovered. Snapshot results whose history publication fails remain
in a bounded 128-entry in-memory pending ledger; publication is retried before
new effects, and the pending SR cannot snapshot again while its result is
unpublished in the same worker process. A blocking-task join failure restores at
least the pending entries that existed before that pass.

This pending ledger is a same-worker barrier, not crash-durable authority. A
daemon/process crash after xe completes but before run-history publication can
lose a newly completed result and repeat the destructive snapshot after restart.
Likewise, restoring the pre-pass map after a panicked blocking task does not
recover a new effect completed inside that task before it panicked; this slice
does not claim that crash/panic window is closed.

Focused hostile coverage proves same-worker late-Bus recovery, retained
configuration/history folding, dynamic schedule discovery, all-read-before-xe
effects, same-worker publication retry without duplicate effects, and shutdown
interrupting startup recovery. A focused async panic-seam regression additionally
proves that a blocking-task join failure does not discard pre-pass pending entries.

## Farm verification

Requested machine 193 address `192.168.23.90` did not accept the farm SSH probe
(`connect timed out`). The repository inventory's canonical address for the same
machine, `172.20.0.90`, was used with explicit
`MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=dc-snap-bus-r42`. No local Cargo
command was run.

Each test used this command shape:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=dc-snap-bus-r42 ssh -i /root/.ssh/mackes_mesh_ed25519 -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 mm@172.20.0.90 'cd ~/magic-mesh-farm-dc-snap-bus-r42 && cargo test -q -p mackesd --features async-services --lib <TEST> -- --exact --nocapture'
```

Results:

- `workers::dc_snap_scheduler::tests::late_bus_replays_retained_authority_and_discovers_dynamic_schedules`: PASS — 1 passed, 0 failed, 4468 filtered out.
- `workers::dc_snap_scheduler::tests::partial_reads_and_publication_failure_defer_without_duplicate_effects`: PASS; landing-correction rerun — 1 passed, 0 failed, 4484 filtered out.
- `workers::dc_snap_scheduler::tests::system_bus_fallback_and_startup_retry_are_shutdown_aware`: PASS — 1 passed, 0 failed, 4468 filtered out.
- `workers::dc_snap_scheduler::tests::blocking_join_failure_restores_pre_pass_pending_results`: PASS — the injected blocking-task panic was observed; 1 passed, 0 failed, 4484 filtered out.

Formatting and scoped diff checks:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=dc-snap-bus-r42 ssh -i /root/.ssh/mackes_mesh_ed25519 -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 mm@172.20.0.90 'cd ~/magic-mesh-farm-dc-snap-bus-r42 && rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/dc_snap_scheduler.rs'
# PASS

git diff --check -- crates/mesh/mackesd/src/workers/dc_snap_scheduler.rs docs/platform/evidence/WL-ARCH-009-2026-08-09-dc-snap-scheduler-bus-recovery-r42.md
# PASS
```

## Source hash

```text
f286e06bbc3ccc8c757226fc91326a0d64267c3575f7e7c1aecc5ac900b10a78  crates/mesh/mackesd/src/workers/dc_snap_scheduler.rs
```

The local and machine-193 source hashes matched. `WORKLIST.md` was not edited;
no commit or push was made.
