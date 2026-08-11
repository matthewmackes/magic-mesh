# WL-ARCH-008 Browser lifecycle request correlation evidence — 2026-08-11

- Scope: Browser VM start/resume polling now requires the terminal workload
  projection to carry the exact request ID published by that lifecycle intent;
  stale or foreign terminal rows cannot complete the current Browser action.
- Focused farm gate on BigBoy (`172.20.0.130`):
  `cargo test -p mde-shell-egui web::tests:: -- --nocapture`.
- Result: **PASS**, 14 passed, 0 failed.
- Boundary: this proves typed request correlation in the shell lifecycle
  controller; guest image quality, live three-seat performance, and physical
  Browser VM acceptance remain open.
