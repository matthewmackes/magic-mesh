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
- Result: **NOT RUN** for this contract extension. At assignment time `.50` was
  2/2 occupied and every free host was below the governed 8 GiB reserve; no
  reserve bypass was attempted. Earlier action-progress behavior remains proven
  separately in `WL-UX-013-2026-08-11-action-result-progress-r262.md`.
- Remaining proof: run the exact contract-aware regression on a warmed safe
  slot, then physical-seat suspend/loss/return and three-seat acceptance.
