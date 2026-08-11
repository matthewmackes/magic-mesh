# WL-FUNC-022 peer convergence probe budget — 2026-08-11

- Scope: Clock peer convergence.
- Change: retry-suppressed peer convergence probes are capped at 512 per tick, independently of the 128 outbound command budget.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func022-convergence-probe-budget-r223 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clock::tests::peer_convergence_probe_budget_bounds_retry_suppressed_work -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
