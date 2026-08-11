# WL-UX-011 device command timeout — 2026-08-11

- Scope: device-control executor.
- Change: fixed helper commands now have a 30-second deadline and `kill_on_drop`, so a stuck module or device helper cannot wedge cancellation and recovery lanes.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux011-command-timeout-r224-final install-helpers/xcp-build.sh cargo test -p mackesd --features async-services workers::device_control::tests::command_control_step_times_out_instead_of_wedging_the_worker -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
