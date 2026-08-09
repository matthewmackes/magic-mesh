# WL-ARCH-009 CA-sign SQLite writer cutover — 2026-08-08

All three residual direct-write sites in `ca/sign.rs` now use the existing typed
writer authority. Certificate signing retains its compare-and-swap/restart
semantics without adding another writer operation or removing operational tests.

## Verification

- `.50`: signing tests passed 18/18 and the process-isolated writer CAS/restart
  proof passed 1/1.
- Local SQLite authority self-test and actual lint passed with two residual
  reviewed sites, down from five.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

One reviewed site each in `workers/host_state.rs` and `workers/job_exec.rs`, plus
live six-group writer failure/restart proof, remain. ARCH-009 stays `Remaining`.
