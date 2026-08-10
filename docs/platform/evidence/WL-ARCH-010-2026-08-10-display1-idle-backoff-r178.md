# WL-ARCH-010 Display1 idle polling — r178

- Scope: native Display1 input polling preserves 5 ms response cadence while active and backs off idle attachment threads in bounded steps to a 25 ms cap, removing the permanent 200 Hz idle loop that can pressure small seats.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-display1-idle-backoff-r178 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::display1_input_poll_sleep_backs_off_only_when_idle -- --nocapture`
- Result: `1 passed; 0 failed; 4711 filtered out` on seat `.50`.
