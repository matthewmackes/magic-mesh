# Clipboard sync Bus recovery — r38

Date: 2026-08-09

Baseline: `a8bd829e727167cc93dd04d4a919d47e89ad6e23`

## Semantics

- Bus root resolution now falls back to `mde_bus::SYSTEM_BUS_ROOT`. An unresolved or unopenable spool no longer terminates the worker: the same worker retries with shutdown-aware exponential backoff bounded from 10 ms through 2 s.
- Legacy CLIP, authenticated consent, VDI envelope V2, collaboration envelope V2, and mesh send are forward/session lanes. On first activation their tail candidates and all V2/mesh replay-ledger seeds are staged before any cursor is installed. A failed tail or seed performs no activation and writes no startup cursor.
- V2 replay-ledger seeds are bounded to the staged activation tail. Messages published between a lane's tail read and ledger seeding remain forward work and are not poisoned as retained replay.
- Existing valid cursors remain authoritative across restart. Startup never rewrites them to a newer tail.
- Target-specific mesh receive remains a durable/fresh-frame lane. It is not tail-primed: its valid checkpoint is preserved, and absent-cursor startup still inspects retained frames after a mandatory replay-ledger seed so signature, expiry, replay, and fresh-frame admission retain their prior semantics.
- Each runtime tick first reads consent, VDI V2, mesh send, target mesh receive, collaboration V2, and CLIP into one effect-free batch. Failure of any lane read defers all lane processing and replicated-head materialization for that tick. Pre/post checks also reject a missing, unstatable, or unreopened replacement Bus index instead of accepting rows from an orphaned SQLite inode as an empty/current view. Successfully read rows are not cursor-advanced until the complete read sweep exists.
- Replicated clipboard history remains durable state. Its current head is observed at activation and only a changed head while the worker is active is materialized locally.

## Changed files

- `crates/mesh/mackesd/src/workers/clipboard_sync.rs`
  - SHA-256: `f665135630533ed16548dec0a5f77bef24161247c5ee946d3840cc2f09c510a4`
- `crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs`
  - SHA-256: `fcd10e16e6e0a4ff9ce570b7231ff454e82177d5cead127f7f40ef646717e9f4`

## Verification

Farm topology was checked first with `./install-helpers/farm-topology.sh table`; BigBoy `172.20.0.130` reported three free heavy slots. All tests and formatting used explicit slot `clipboard-sync-bus-r38`.

Initial compile and exact tests used:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=clipboard-sync-bus-r38 ./install-helpers/xcp-build.sh cargo test -q -p mackesd --features async-services --lib workers::clipboard_sync::tests::clipboard_activation_is_atomic_and_preserves_mesh_receive_semantics -- --exact --nocapture
PASS: 1 passed; 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=clipboard-sync-bus-r38 ./install-helpers/xcp-build.sh cargo test -q -p mackesd --features async-services --lib workers::clipboard_sync::tests::clipboard_bus_recovery_skips_retained_and_defers_failed_sweep -- --exact --nocapture
PASS: 1 passed; 0 failed
```

Farm rustfmt:

```text
ssh mm@172.20.0.130 'cd /home/mm/magic-mesh-farm-clipboard-sync-bus-r38 && cargo fmt -- crates/mesh/mackesd/src/workers/clipboard_sync.rs crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs'
PASS
```

After formatting, the same two new exact tests passed again. Existing narrow semantic regressions also passed:

```text
cargo test -q -p mackesd --features async-services --lib workers::clipboard_sync::tests::durable_cursor_resumes_after_restart_without_replaying_retained_lane -- --exact --nocapture
PASS: 1 passed; 0 failed

cargo test -q -p mackesd --features async-services --lib workers::clipboard_sync::mesh::tests::restart_seed_rejects_a_generation_already_forwarded_to_canonical_authority -- --exact --nocapture
PASS: 1 passed; 0 failed
```

The latter farm commands were run directly in the same explicit BigBoy slot after restoring only that disposable slot's unrelated `dc_snap_scheduler.rs` to baseline. The shared workspace contains an in-progress concurrent `dc_snap_scheduler.rs` edit that currently fails to compile because `write_run` is missing; it was not modified or reverted locally.

Scoped farm and local whitespace checks:

```text
git diff --no-index --check /dev/null crates/mesh/mackesd/src/workers/clipboard_sync.rs
git diff --no-index --check /dev/null crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs
git diff --check -- crates/mesh/mackesd/src/workers/clipboard_sync.rs crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs
PASS: no output
```

Only pre-existing crate-wide warnings were emitted. No commit was created and `docs/platform/WORKLIST.md` was not edited.
