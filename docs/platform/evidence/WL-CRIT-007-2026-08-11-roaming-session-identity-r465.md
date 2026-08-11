# WL-CRIT-007 roaming-session identity — 2026-08-11

- Scope: roaming recovery binds retained sessions to one canonical identity.
- Hostile boundary: restart cannot adopt a substituted duplicate session identity.
- Focused gate: `cargo test -p mackesd workers::session_roaming::tests::restarted_roaming_cannot_adopt_a_substituted_duplicate_session_identity -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live detach/reconnect proof across node sleep.
