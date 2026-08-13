# WL-ARCH-009 reconcile generation ownership — r513

## Result

The reconcile worker now holds process-local exclusive ownership keyed by the
normalized durable SQLite-store path. A second live generation for the same
store fails closed before its thread starts, preventing duplicate repair probes
and duplicate audit publication. The RAII lease is released on normal exit and
unwind, so a stopped generation cannot permanently block its replacement.
Independent stores retain independent ownership and may run concurrently.

## Farm verification

- `.50` (`172.20.0.50`), slot `arch009-reconcile-owner-final`:
  `cargo test -p mackesd spawn_reconcile_worker_rejects_duplicate_live_generation_and_releases_on_exit -- --nocapture`
  passed 1/1 with 4,956 filtered. The hostile duplicate panicked at admission as
  expected; a replacement generation started and exited after the first joined.
- `.170` (`172.20.0.170`), slot `arch009-reconcile-clippy2`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.50`, slot `arch009-reconcile-fmt2`: direct file-scoped
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/worker.rs` passed.
- `git diff --check` passed.

The first BigBoy attempt was interrupted and is not counted as evidence. No
duplicate rerun was started there. Workspace-wide formatting exposed unrelated
existing drift and is likewise not represented as this slice's result.

## Remaining acceptance

First-release package integration and the deferred post-release one-node
process/cgroup census, crash/Bus-loss recovery, bounded snapshot convergence,
Workers/Action Console ownership, and installed-seat corrected-forward proof
remain for WL-ARCH-009.
