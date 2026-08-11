# WL-FUNC-021 Navidrome setup timeout — 2026-08-11

- Scope: Navidrome systemctl and re-provision commands share a 15-second bounded subprocess deadline.
- Farm: `.90` `172.20.0.90`, slot `navidrome-setup-bound-r232`.
- Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=navidrome-setup-bound-r232 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::navidrome_supervisor -- --nocapture`.
- Result: PASS, 3 passed, 0 failed.
