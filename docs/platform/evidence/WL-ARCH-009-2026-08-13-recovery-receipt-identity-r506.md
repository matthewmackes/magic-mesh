# WL-ARCH-009 recovery receipt identity — r506

Date: 2026-08-13

## Result

`recovery::execute` now treats the immutable node and mesh in the recovery plan
as the authority for completion. A successful apply seam may no longer return a
receipt for another node or mesh and have that stale/foreign result published as
`RecoveryOutcome::Reenrolled`; mismatches fail closed as a typed
`reenroll-receipt` error.

This closes a corrected-forward recovery gap at the process/remote enrollment
seam without changing worker modules, shared contracts, Cargo metadata, or the
canonical worklist.

## Farm gates

- `.90`, slot `arch009-recovery-test`:
  `cargo test -p mackesd --lib execute_rejects_stale_or_foreign_recovery_receipts -- --nocapture`
  — passed 1/1 with 4,954 filtered out.
- `.170`, slot `arch009-recovery-clippy`:
  `cargo clippy -p mackesd --lib -- -D warnings` — passed.
- `.50`, slot `arch009-recovery-filefmt`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/recovery.rs` — passed.
- `git diff --check` — passed for the shared worktree.

The crate-wide format check was not claimed: it reached unrelated pre-existing
format drift outside this slice. The file-scoped gate above covers the owned Rust
file without modifying concurrent work.

## Remaining acceptance

WL-ARCH-009 still requires first-release package integration and the deferred
post-release one-node process/cgroup census, crash and Bus-loss recovery,
bounded snapshot convergence, Workers/Action Console route ownership, and
installed-seat corrected-forward recovery proof. This focused source invariant
does not claim those live acceptance rows.
