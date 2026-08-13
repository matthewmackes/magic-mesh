# WL-ARCH-009 — authenticated Worker change-set Bus consumer (r552)

Date: 2026-08-13

Commit: recorded by the commit that adds this evidence.

## Result

The Actions process now owns a production consumer for
`action/workers/change-set/<node>`. For every bounded retained request it:

- decodes the closed `WorkerChangeSetRequest` contract;
- binds `ActionAuthorizer` to the exact retained body, local node identity,
  `workers-change-set` verb, and `change-set:<request_id>` target;
- resolves every requested worker and one exact generation from the
  observation-owned aggregate runtime status;
- dispatches only an admitted body to the generation-bound
  `WorkerChangeSetExecutor`; and
- retains a validated `WorkerChangeSetResult` on
  `state/workers/change-set/<node>`.

Wrong-body, wrong-identity, and replayed capabilities are refused before the
executor can stage or mutate work. The authorization nonce is claimed through
the existing durable replay ledger. Malformed bodies without a trustworthy
typed request identity do not produce synthetic results. A missing current
runtime aggregate also fails closed.

No authenticated mutation provider exists yet. `Commit` therefore continues
to return the explicit typed refusal `no authenticated mutation handler is
registered`; this slice does not fabricate a handler or success.

## Files

- `crates/mesh/mackesd/src/worker_change_set.rs`
- `crates/mesh/mackesd/src/bin/mackesd/spawn.rs`

## Farm gates

- `172.20.0.90`, slot `1`:
  `cargo test -p mackesd --lib --features async-services worker_change_set::tests::bus_admission_rejects_wrong_body_identity_and_replay_before_dispatch -- --exact --nocapture`
  — passed, 1 passed / 0 failed / 4,998 filtered.
- `172.20.0.90`, slot `1`:
  `cargo clippy -p mackesd --all-targets --all-features -- -D warnings`
  — passed in 4m42s.
- `172.20.0.130`, slot `3`:
  `cargo build -p mackesd --all-targets --all-features`
  — passed in 8m23s.
- `172.20.0.196`, slot `1`:
  `cargo fmt -p mackesd -- --check`
  — found one owned wrap in `spawn.rs` and unrelated pre-existing formatting
  drift elsewhere in the crate; the owned wrap was corrected exactly. No
  package-wide mechanical rewrite was applied.
- Scoped `git diff --check` — passed.

The initial `.50/1` build route refused before sync because `/home` had only
5.2 GiB free versus the helper's 8 GiB safety floor. It was replaced, not
duplicated, by the successful BigBoy build above. Cold or lock-contended test
attempts on `.170` and `.196` were stopped before the final unique `.90/1`
focused gate.

## Remaining ARCH-009 acceptance

- Add a canonical action descriptor only together with an existing
  authenticated, generation-aware mutation provider, then register that
  provider with the executor.
- Continue retiring legacy Fleet/Workbench/This Node route aliases where a
  typed Workers leaf already exists.
- Cut and install the first full release.
- After that release, run the deferred non-blocking one-node Action Console,
  lifecycle, restart, and direct-DRM acceptance.
