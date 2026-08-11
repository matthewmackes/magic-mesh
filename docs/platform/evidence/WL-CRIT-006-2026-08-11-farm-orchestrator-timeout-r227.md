# WL-CRIT-006 farm orchestrator timeout — 2026-08-11

- Scope: etcd range/get curl calls use bounded command timeouts, reap hung children, and fail closed on timeout or non-success status.
- Farm: BigBoy `172.20.0.130`, slot `farm-orchestrator-timeout-r227`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=farm-orchestrator-timeout-r227 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::farm_orchestrator::tests::hung_curl_returns_no_jobs_or_value -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
