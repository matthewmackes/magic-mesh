# WL-FUNC-011 CAS read-only replay authority — 2026-08-11

- Scope: canonical collaboration CAS blobs are sealed read-only before publication, including idempotent replay of an existing inode.
- Hostile boundary: concurrent inode substitution during replay sealing fails closed and cannot leave canonical bytes owner-writable.
- Focused gate: `cargo test -p mde-collab-core blob::tests::retained_blob_replay_cannot_leave_canonical_bytes_owner_writable -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 11,220,112 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 113 filtered out.
- Remaining boundary: live replicated CAS commit/recovery proof remains.
