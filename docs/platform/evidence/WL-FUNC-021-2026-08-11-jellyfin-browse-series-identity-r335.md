# WL-FUNC-021 Jellyfin browse series identity — 2026-08-11

- Scope: seasons and episodes must carry the exact requested series identity
  before entering the browse tree.
- Hostile boundary: foreign or stale items, including colliding season IDs,
  cannot substitute the requested series state.
- Focused gate: `cargo test -p mde-jellyfin browse::tests::foreign_series_items_cannot_substitute_browse_tree_state -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 8,457,532 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 96 filtered out.
- Remaining boundary: live remote browse/outage and package proof remain.
