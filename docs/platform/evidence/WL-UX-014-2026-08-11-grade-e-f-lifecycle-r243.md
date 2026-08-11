# WL-UX-014 grade E/F lower-third lifecycle — 2026-08-11

- Scope: the shared production `ToastHost` derives its close control from the
  governed dwell lifecycle rather than Critical severity alone. Grade E keeps
  its 15-second countdown and uses Dismiss; grade F remains held until explicit
  Acknowledge. Both retain Critical preemption.
- Production path: health authority → `event/toast/show` →
  `HealthKironAlert` decoder → shared `ToastHost` → lower-third controls.
- Farm: `172.20.0.90`, slot `1`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mde-egui toast::tests::grade_e_timed_critical_cannot_enter_grade_f_acknowledgement_lifecycle -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 291 filtered out.
