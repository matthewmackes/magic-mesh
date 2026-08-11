# WL-FUNC-021 Navidrome command timeout — 2026-08-11

- Scope: Navidrome/systemd setup and health subprocesses.
- Change: systemctl and setup-helper commands use the shared timeout-bounded subprocess helpers and fail closed on deadline expiry.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-navidrome-timeout-20260811-bigboy install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::media_navidrome::tests::bounded_command_timeout_fails_closed_and_reaps_child -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
