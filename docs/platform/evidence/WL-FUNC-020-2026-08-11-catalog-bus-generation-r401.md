# WL-FUNC-020 Android catalog Bus generation — 2026-08-11

- Scope: signed Android catalog replay and import progress must commit to one live Bus index generation.
- Hostile boundary: pathname replacement after open cannot strand publication on a retired SQLite inode while advancing cursor/current state for the replacement generation.
- Focused gate: `cargo test -p mackesd workers::android_catalog::tests::replacement_after_open_cannot_strand_catalog_replay_on_retired_index -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 13,636,188 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,868 filtered out.
- Remaining boundary: live importer index replacement, governed action recovery, and nested-KVM Cuttlefish lifecycle proof remain.
