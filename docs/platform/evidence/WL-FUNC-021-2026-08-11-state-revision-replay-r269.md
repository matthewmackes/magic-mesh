# WL-FUNC-021 Music state revision replay evidence — 2026-08-11

- Scope: Music's durable global authority and per-peer projection now reject a
  recovered in-memory state older than either existing durable record.
- Boundary: `updated_ms` is the revision. A lower revision or a different body
  at the same revision fails before either projection is written; an exact
  same-revision replay remains idempotent. This prevents stale restart memory
  from restoring obsolete `playing: true` state over a newer stopped record.
- Regression: `state::tests::stale_revision_replay_after_restart_preserves_newer_durable_state`
  proves both durable projections retain the newer state under rollback and
  same-revision equivocation.
- Intended farm command: `cargo test -p mde-musicd --lib state::tests::stale_revision_replay_after_restart_preserves_newer_durable_state -- --exact --nocapture`.
- Result: **PASS** on BigBoy, slot 3 — 1 passed, 0 failed, 247 filtered. The
  first attempt was capacity-blocked and a later sync attempt saw rsync code 24;
  neither emitted a Cargo result. The clean warmed rerun above supersedes them.
  Targeted `git diff --check` passed.
- Remaining proof: retain the epic's physical renderer/two-seat
  corrected-forward acceptance.
