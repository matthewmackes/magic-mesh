# WL-FUNC-016 clipboard consent-epoch revocation — 2026-08-11

- Scope: clipboard envelopes must originate within the currently admitted consent epoch.
- Hostile boundary: revocation/re-enable cannot resurrect retained prior-epoch content; stale rows retire without blocking fresh ordered materialization.
- Focused gate: `cargo test -p mackesd workers::clipboard_sync::tests::reenabled_session_cannot_apply_clipboard_from_a_revoked_consent_epoch -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 14,557,452 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,863 filtered out.
- Remaining boundary: live peer/VDI consent revocation, reconnect, and daemon-restart proof remain.
