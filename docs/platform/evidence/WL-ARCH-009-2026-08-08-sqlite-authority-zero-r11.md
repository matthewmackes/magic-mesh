# WL-ARCH-009 SQLite authority reaches zero — 2026-08-08

The final direct-write sites in `workers/host_state.rs` and
`workers/job_exec.rs` now use the single process-isolated typed writer. The
reviewed baseline is empty, and the authority lint accepts that empty inventory
while still rejecting any new direct SQLite write outside the owner.

## Verification

- `.50`: host-state passed 19/19, job executor 3/3, writer restart 1/1, and
  writer compare-and-swap/replay 1/1; total 24/24.
- SQLite authority self-test passed and the actual lint passed at zero sites.
- Scoped rustfmt and `git diff --check` passed.
- No operational tests were removed.

## Remaining acceptance gap

The SQLite portion of ARCH-009 is complete. Workers/Action Console cutover,
six-group live failure/resource proof, legacy route deletion, and fleet
convergence remain, so the epic stays `Remaining`.
