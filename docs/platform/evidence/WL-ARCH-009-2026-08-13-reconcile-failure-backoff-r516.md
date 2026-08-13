# WL-ARCH-009 reconcile failure backoff — r516

## Acceptance link

This closes a concrete portion of WL-ARCH-009 S4, whose deliverable explicitly
requires `shutdown, queue, retry, watchdog, and cgroup tests` and whose done
condition requires all six groups to `start/stop/recover under declared
budgets`. The reconcile worker previously retried a persistently unavailable or
corrupt store every healthy 30-second cadence forever. That was an unbounded
failure retry budget, not speculative cleanup.

Consecutive failed reconcile ticks now back off from 30 seconds through 60,
120, and 240 seconds to a five-minute cap. Arithmetic saturates for hostile
failure counts, shutdown continues to interrupt every wait, and the first
successful tick resets the failure generation so a later outage starts at the
normal cadence.

This does not duplicate the generation ownership landed in `7d372caf`.
`spawn_reconcile_worker_rejects_duplicate_live_generation_and_releases_on_exit`
covers exclusive process-local ownership. The existing
`spawn_reconcile_worker_exits_when_shutdown_flips` and
`interruptible_sleep_returns_when_flag_flips_mid_sleep` tests cover healthy
shutdown and wait interruption. None covered consecutive failed ticks, delay
growth, the retry cap, overflow, or recovery reset.

## Farm verification

- `.196` (`172.20.0.196`), workspace
  `magic-mesh-farm-arch009-reconcile-backoff-test-reroute`:
  `cargo test -p mackesd reconcile_failure_backoff_is_exponential_bounded_and_restart_safe -- --nocapture`
  passed 1/1 with 4,959 filtered out.
- `.170` (`172.20.0.170`), workspace
  `magic-mesh-farm-arch009-reconcile-backoff-clippy-final`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.90` (`172.20.0.90`), workspace
  `magic-mesh-farm-arch009-reconcile-backoff-fmt`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/worker.rs` passed.
- `git diff --check` passed.

The stale `.50` copy was terminated by exact process identity after contention
was discovered and is excluded from evidence. BigBoy was not used.

## Remaining acceptance

ARCH-009 still requires first-release package integration and the deferred
post-release one-node process/cgroup census, crash and Bus-loss recovery,
bounded snapshot convergence, Workers/Action Console route ownership, and
installed-seat corrected-forward proof.
