# WL-UX-013 firewall journal bound — 2026-08-11

- Scope: `journalctl` uses the shared timeout/capture boundary; oversized output is rejected before the cursor advances.
- Farm: BigBoy `172.20.0.130`, slot `ux013-firewall-journal-bound-r230`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux013-firewall-journal-bound-r230 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::firewall_monitor::tests::oversized_journal_output_is_rejected_before_cursor_progress -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
