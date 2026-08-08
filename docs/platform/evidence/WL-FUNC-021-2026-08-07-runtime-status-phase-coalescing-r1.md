# WL-FUNC-021 — runtime-status phase jitter and coalescing (2026-08-07)

## Change

The shared `mackesd` worker-runtime status publisher now assigns each node a
stable phase within the existing five-second cadence. The phase is derived
from the node identity, so daemon restarts do not synchronize every seat back
onto one wakeup boundary. Rejected-sample retries retain the existing bounded
5/10/20/40/60-second ladder with a small node-stable offset.

Unchanged lifecycle projections are coalesced: the runtime file and retained
Bus lanes are written immediately for a semantic worker change and at most
every ten seconds otherwise. Ten seconds stays below the projection's
15-second freshness window. The coalescer records a sample only after the
publication attempt succeeds, so failed writes remain eligible for retry.

Unknown names in the shared supervisor map are ignored for publication rather
than aborting the complete aggregate. Registered rows remain fully validated,
deterministically ordered, and published under their existing worker/node
topics; unknown rows are never emitted.

## Farm verification

- BigBoy `.130`, slot `runtime-status-jitter-r2`:
  `cargo test -p mackesd worker_runtime_status -- --nocapture` — **15 passed,
  0 failed** (4,383 filtered).
- `.50`, slot `runtime-status-syntax-r1`:
  `cargo check -p mackesd --lib --locked` — **passed**.
- `git diff --check` for the scoped source paths — **passed**.

The local orchestration host does not have `rustfmt` installed. A package-wide
formatter gate was not used because the package contains unrelated dirty
formatting; the farm compile/test gate covers the changed Rust syntax and
behavior.

## Remaining proof

This is source-level mitigation and farm-verified behavior. It has not yet
been installed on Dell or measured against the live multi-seat CPU trace;
those runtime proofs remain blocked while Dell is unreachable.
