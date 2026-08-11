# WL-FUNC-011 CAS purge inode binding — 2026-08-11

- Scope: destructive CAS purge remains bound to verified content and the canonical inode.
- Hostile boundary: restart plus concurrent non-CAS replacement cannot make purge unlink substituted bytes.
- Focused gate: `cargo test -p mde-collab-core blob::tests::restarted_purge_cannot_unlink_a_concurrent_non_cas_replacement -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 12,475,912 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 115 filtered out.
- Remaining boundary: replace a replicated live CAS path during purge and prove only the verified canonical inode may be removed.
