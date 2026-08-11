# WL-FUNC-016 consent checkpoint failure — 2026-08-11

- Scope: clipboard consent becomes usable only after its durable cursor is committed; a failed checkpoint clears provisional in-memory authorization.
- Hostile boundary: blocking the atomic consent-cursor write cannot authorize a queued rich clipboard envelope in-process or after restart.
- Focused gate: `cargo test -p mackesd workers::clipboard_sync::tests::failed_consent_checkpoint_cannot_authorize_clipboard_before_or_after_restart -- --exact --nocapture`.
- Farm: clean coordinator-only run on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,884 filtered out.
- Remaining boundary: exercise the same failure and repaired restart against live VDI clipboard transport and its production Bus filesystem.
