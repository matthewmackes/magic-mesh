# WL-ARCH-009 runtime-status ancestor integrity — 2026-08-13

## Scope

The six isolated `mackesd` process groups exchange credential-free runtime
snapshots through disjoint files before the aggregate owner folds them into the
node projection. `worker_runtime_status` previously rejected a symlink at the
status file and its immediate parent, but an earlier directory component could
still be a symlink. That allowed a group publisher or the aggregate reader to
escape the intended runtime-status directory through an ancestor redirect.

The runtime file boundary now walks every directory component without following
symlinks. Missing write-side directories are created and admitted one component
at a time; parent traversal, non-directory components, and symlinked ancestors
fail closed. The read side applies the same ancestor admission before opening a
group snapshot. The focused hostile regression proves both write and read reject
a redirected group-status ancestor.

## Farm gates

- **PASS — focused test, BigBoy `.130`, slot
  `arch009-runtime-ancestor-test-20260813`:**
  `cargo test -p mackesd --locked group_runtime_files_are_disjoint_and_bounded_on_read --lib -- --nocapture`
  completed with `1 passed; 0 failed; 4925 filtered out`.
- **PASS — strict library clippy, `.170`, slot
  `arch009-runtime-ancestor-clippy-20260813`:**
  `cargo clippy -p mackesd --locked --lib -- -D warnings` completed successfully.
- **PASS — file-scoped formatting, `.196`, slot
  `arch009-runtime-ancestor-filefmt-20260813`:** after
  `install-helpers/xcp-build.sh sync`,
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/worker_runtime_status.rs`
  completed successfully.

The broader package fmt check on `.50` was not used as evidence because it found
pre-existing formatting drift in the concurrently modified
`src/bin/mackesd/spawn.rs`; this slice did not edit or format that file.

## Remaining acceptance

This closes one process-isolation ownership escape in the six-group runtime
projection. WL-ARCH-009 still requires the broader Workers UI ownership/cutover
and removal of duplicate surfaces. Fleet, package, and installed live proof
remain post-release according to the active worklist.
