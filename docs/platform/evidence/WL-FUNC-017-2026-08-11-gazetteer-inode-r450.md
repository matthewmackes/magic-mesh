# WL-FUNC-017 gazetteer inode — 2026-08-11

- Scope: offline gazetteer admission rejects multiply-linked files before and after descriptor opening.
- Hostile boundary: a second writable path cannot retain mutation authority over the SQLite navigation source.
- Focused gate: `cargo test -p mde-maps-location-egui --lib geocode::tests::hardlinked_gazetteer_cannot_alias_navigation_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 312 filtered out.
- Remaining boundary: prove a live offline route stays bound to the admitted gazetteer across provider restart.
