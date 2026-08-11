# WL-UX-013 health action result progress — 2026-08-11

- Scope: after the Health modal publishes an exact governed request, it polls a
  bounded per-request result lane and accepts only a schema-, request-,
  condition-, action-, generation-, timestamp-, and publisher-bound terminal
  result. Node actions require requester equals target; mesh actions bind the
  worker publisher to the local requester rather than accepting a fabricated
  `health:mesh` identity.
- Presentation: stale or unrelated rows cannot clear pending state. Applied
  results wait for fresh health at the result generation; if the exact scoped
  condition remains active, the modal reports partial failure with current
  evidence instead of claiming recovery. Duplicate controls remain disabled
  only while the action is genuinely pending.
- Farm: `172.20.0.50`, slot `2`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mde-shell-egui
  health_modal::tests::action_result_progress_binds_identity_generation_target_and_reports_partial_failure
  -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 1,550 filtered out. The disposable slot-2
  build output was removed afterward.
- Remaining proof: physical-seat suspend/loss/return recovery and final
  three-seat acceptance.
