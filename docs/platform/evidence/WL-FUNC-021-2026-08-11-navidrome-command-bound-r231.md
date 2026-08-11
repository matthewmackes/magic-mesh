# WL-FUNC-021 Navidrome command bound — 2026-08-11

- Scope: Navidrome supervisor systemctl calls use the shared timeout/output boundary with a 15-second deadline.
- Farm: BigBoy `172.20.0.130`, slot `navidrome-hardening-0811`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=navidrome-hardening-0811 cargo test -p mackesd --features async-services navidrome_supervisor`.
- Result: PASS, 3 passed, 0 failed.
