# WL-UX-013 firewall history bound — 2026-08-11

- Scope: firewall JSONL retention requires a regular file and caps history reads at 4 MiB before trimming or rewriting.
- Farm: BigBoy `172.20.0.130`, slot `ux013-firewall-history-bound-r228`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux013-firewall-history-bound-r228 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::firewall_monitor::tests::trim_rejects_oversized_history_before_reading_or_rewriting -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
