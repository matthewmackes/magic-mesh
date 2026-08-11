# WL-ARCH-009 DC health probe timeout — 2026-08-11

- Scope: DC health Dom0 SSH probing uses the shared 15-second timeout and bounded stream capture.
- Farm: BigBoy `172.20.0.130`, slot `dc-health-dom0-timeout-r233`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=dc-health-dom0-timeout-r233 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::dc_health::tests::dom0_probe_command_times_out_a_hung_child -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed.
