# WL-ARCH-010 VDI input-generation authority — 2026-08-11

- Scope: replacement, disconnect, retry, and resize re-dial revoke guest input authority while prior-generation frames remain display-only.
- Hostile boundary: a replacement arriving before an old decoded frame is uploaded cannot restore input; only a fresh current-transport frame can.
- Focused gate: `cargo test -p mde-shell-egui vdi::presentation_authority_tests::replacement_request_revokes_stale_frame_input_authority_until_new_frame -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 17.0 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 1,557 filtered out.
- Remaining boundary: live reconnect/resize input and direct-DRM proof remain.
