# WL-UX-013 health lifecycle provenance — 2026-08-13

- Scope: bind locally recovered condition history, acknowledgement, and snooze
  state to complete condition provenance rather than the display id alone.
- Production boundary: a same-id condition with a substituted scope,
  component, source, requirement class, or evidence provider starts a new
  incident. The prior incident is retained as resolved history with its final
  positive observation; authority state does not cross providers.
- Regression:
  `workers::node_grade::tests::lifecycle_provenance_substitution_starts_new_incident_and_preserves_history`.
- Farm focused test: BigBoy `172.20.0.130`, slot 3. Corrected exact command
  `cargo test -p mackesd workers::node_grade::tests::lifecycle_provenance_substitution_starts_new_incident_and_preserves_history -- --exact --nocapture`
  passed `1/1` with 4,998 tests filtered.
- Farm library check: `172.20.0.50`, slot 1. Corrected command
  `cargo check -p mackesd --lib --all-features` passed in 2m44s.
- The earlier failed compile was an invalid pre-correction fixture run and is
  retained only as historical context; it is superseded by the corrected gates
  above.
- Farm formatting: `172.20.0.170`, slot 1. The one crate-wide check was red from
  pre-existing unrelated formatting drift outside `node_grade.rs`; those files
  were preserved.
- Scoped `git diff --check`: passed after the final correction.
- Remaining verification: execute the exact hostile regression and compile the
  corrected source in a later permitted gate wave. Live health transition and
  recovery acceptance remains deferred until after the first full release.
