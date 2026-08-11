# WL-ARCH-009 Netdata overlay-IP bound — 2026-08-11

- Scope: Netdata aggregator overlay-IP input requires a regular file and is capped at 256 bytes before trimming.
- Farm: BigBoy `172.20.0.130`, slot `arch009-netdata-overlay-bound-r231`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-netdata-overlay-bound-r231 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::netdata_aggregator::tests::oversized_overlay_ip_file_fails_closed_before_trim -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
