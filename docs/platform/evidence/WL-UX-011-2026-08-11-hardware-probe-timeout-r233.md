# WL-UX-011 hardware probe timeout — 2026-08-11

- Scope: hardware inventory command probes use the shared 15-second timeout and fail closed when a child hangs.
- Farm: BigBoy `172.20.0.130`, slot `hardware-probe-timeout-r233`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=hardware-probe-timeout-r233 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::hardware_probe::tests::command_probe_times_out_a_hung_child -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed.
