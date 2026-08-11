# WL-FUNC-011 — alert action ID admission (r210)

- Scope: `RunAlertAction` rejects empty, oversized, control, slash, and
  backslash IDs before lookup or signing.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func011-alert-action-id-admission-r210-final install-helpers/xcp-build.sh cargo test -p mde-collab-core --lib 'tests::alert_action_id_admission_rejects_unbounded_or_path_bearing_ids' -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out`; format passed on `.50`.
