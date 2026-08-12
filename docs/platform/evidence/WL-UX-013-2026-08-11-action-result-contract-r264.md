# WL-UX-013 health action result contract — 2026-08-11

- Scope: `HealthActionResult` now rejects unknown fields and provides one
  intrinsic validation boundary for schema, bounded identifiers, detail/audit
  text, secret-shaped content, completion time, nonzero generation, and nested
  refreshed evidence. Node-grade enforces it before journal replay, publication,
  result acknowledgement, and new result persistence; Health modal polling
  enforces it before request/publisher binding or presentation.
- Hostile coverage: malformed JSON, unsupported fields, oversized detail,
  invalid identifiers, future completion/evidence, and invalid retained rows
  cannot be published, acknowledged, replayed, or presented.
- Intended focused gate: `install-helpers/xcp-build.sh cargo test -p
  mde-shell-egui
  health_modal::tests::action_result_progress_binds_identity_generation_target_and_reports_partial_failure
  -- --exact --nocapture`.
- Result: **PASS**. Farm `.90`, slot `ux013-action-result`, ran the exact
  contract-aware regression after a full test-profile compilation: 1 passed,
  0 failed, 1,577 filtered. Farm `.90`, slot `ux013-clippy-bin`, ran
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui` to completion with
  warnings only (1,339 warnings). The earlier `--lib` probe was invalid because
  this package exposes a binary target only and is not counted as a gate.
- Remaining proof: physical-seat suspend/loss/return and three-seat acceptance.
