# WL-ARCH-009 scheduler shutdown gate — 2026-08-13

- Scope: the supervised scheduler worker must initialize its durable outbox and exit promptly when its shutdown token is signaled.
- Fix: the shutdown fixture now supplies an isolated temporary state root, so the test exercises the worker loop instead of failing before startup because the default state path is unavailable.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-scheduler-shutdown-fixture-20260813 install-helpers/xcp-build.sh cargo test -p mackesd --locked run_loop_exits_promptly_on_shutdown -- --nocapture`.
- Result: **PASS**, 7 related shutdown tests passed, 0 failed; farm `.90`.
- Clippy gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-scheduler-clippy-20260813 install-helpers/xcp-build.sh cargo clippy -p mackesd --locked --lib` exited 0 with existing warnings only.
- Full coding gate remains open: the prior complete mackesd suite had 4,902 passes and 23 failures; this slice addresses the scheduler lifecycle fixture only.
