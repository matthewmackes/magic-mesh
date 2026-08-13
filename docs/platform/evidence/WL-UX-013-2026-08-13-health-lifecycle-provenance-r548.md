# WL-UX-013 health lifecycle provenance — 2026-08-13

- Scope: bind locally recovered condition history, acknowledgement, and snooze
  state to complete condition provenance rather than the display id alone.
- Production boundary: a same-id condition with a substituted scope,
  component, source, requirement class, or evidence provider starts a new
  incident. The prior incident is retained as resolved history with its final
  positive observation; authority state does not cross providers.
- Regression:
  `workers::node_grade::tests::lifecycle_provenance_substitution_starts_new_incident_and_preserves_history`.
- Farm focused test: BigBoy `172.20.0.130`, slot 3. The exact selector was
  discovered by the module-qualified test binary, but compilation stopped on
  the initial patch because `RequirementClass` is not `Ord` and the fixture
  named a nonexistent `HealthComponent::Services`. Both defects were corrected
  by using bounded vector equality and `HealthComponent::System`; cadence
  prohibited a rerun, so this run is **not** claimed as passing evidence.
- Farm strict Clippy: `172.20.0.170`, slot 1. The same pre-correction `Ord`
  compile error stopped the one allowed run; no Clippy diagnostic was emitted
  for the production behavior.
- Farm build: `172.20.0.170`, slot 2. The same pre-correction `Ord` error and
  invalid fixture variant stopped the one allowed run.
- Farm formatting: `172.20.0.170`, slot 1. The one crate-wide check was red from
  pre-existing unrelated formatting drift outside `node_grade.rs`; those files
  were preserved.
- Scoped `git diff --check`: passed after the final correction.
- Remaining verification: execute the exact hostile regression and compile the
  corrected source in a later permitted gate wave. Live health transition and
  recovery acceptance remains deferred until after the first full release.
