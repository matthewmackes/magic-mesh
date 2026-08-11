# WL-FUNC-011 import-map inode — 2026-08-11

- Scope: native collaboration migration/replay-map reads stay bound to the checked single-link, non-symlink inode.
- Hostile boundary: a hard-linked import map cannot preserve an external alias capable of mutating replay authority.
- Focused gate: `cargo test -p mde-collab-core import::tests::import_map_hardlink_cannot_alias_replay_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 116 filtered out.
- Remaining boundary: exercise a migrated live collaboration space with a replaced legacy map and prove no event replay or duplication.
