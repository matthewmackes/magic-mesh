# WL-UX-011 inventory staging nofollow — 2026-08-11

- Scope: device-inventory publication stages through an exclusive newly created regular-file descriptor.
- Hostile boundary: pre-planted symlinks or multiply-linked staging rows cannot redirect bytes into an external target.
- Focused gate: `cargo test -p mackesd workers::device_inventory::tests::inventory_publish_cannot_follow_a_substituted_staging_row -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 19,567,036 KiB free.
- Result: **PASS**, clean post-review rerun: 1 passed, 0 failed, 4,879 filtered out.
- Remaining boundary: substitute the staging row during live replicated publication and verify neither target nor inventory is changed.
