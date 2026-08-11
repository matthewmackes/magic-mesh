# WL-FUNC-022 command-generation-loss recovery evidence — 2026-08-11

- Scope: a Clock command may commit its SQLite authority and request identity
  before transient Bus publication fails. The worker now reloads that durable
  winner immediately and marks publication pending instead of restoring stale
  in-memory state.
- Hostile boundary: after the commit/publication split, the regression replaces
  the Bus index with an empty generation so the original command no longer
  exists to replay. Clock publishes the durable revision corrected-forward,
  without reapplying the command or duplicating its effects.
- Focused gate: `cargo test -p mackesd --features async-services workers::clock::tests::command_commit_survives_publication_failure_and_bus_generation_loss -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,827 filtered out.
- Remaining boundary: this closes the command-generation-loss seam only. The
  separate deadline-publication regression in r267 still needs its exact gate,
  and the epic's live multi-peer acceptance remains open.
