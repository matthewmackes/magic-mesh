# WL-FUNC-016 invalid clipboard replacement revocation — 2026-08-11

- Scope: a rejected oversized or invalid local replacement revokes the prior offer and queued request authority.
- Hostile boundary: replacement failure cannot leave stale clipboard bytes available for later guest disclosure.
- Focused gate: `cargo test -p mde-vdi-rdp --features live-connect clipboard::tests::rejected_oversized_replacement_revokes_stale_host_clipboard_authority -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 8,541,000 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 104 filtered out.
- Remaining boundary: live cross-session rich clipboard recovery proof remains.
