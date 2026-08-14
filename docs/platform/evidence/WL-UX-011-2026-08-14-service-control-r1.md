# UX-011 service-control provider boundary (2026-08-14)

- **Implementation:** `DeviceControlOp::RestartService` is a typed operation
  on the existing generation-bound, capability-authorized, audited device
  control rail. The shell exposes it only for `services` rows; hardware rows
  retain the four hardware verbs.
- **Safety boundary:** the daemon accepts only a provider-admitted service
  category and a bounded `.service` unit identifier, then emits the fixed
  `systemctl restart <unit>` argv. Cross-category and control-bearing unit
  names fail before execution.
- **BigBoy daemon gate:**
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux011-service-control-types install-helpers/xcp-build.sh cargo test -p mackesd --features async-services workers::device_control::tests -- --nocapture`
  — **30 passed, 0 failed**.
- **BigBoy shell gate:**
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux011-service-control-shell install-helpers/xcp-build.sh cargo check -p mde-shell-egui --all-targets`
  — **passed**; only the pre-existing `begin_connection_generation`
  dead-code warning remains.
- This is implementation evidence only; physical service recovery and
  installed-seat rollout proof remain owned by `WL-TEST-001`.
