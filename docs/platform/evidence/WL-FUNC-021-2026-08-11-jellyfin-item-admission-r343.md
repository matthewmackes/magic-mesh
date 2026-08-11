# WL-FUNC-021 Jellyfin item admission — 2026-08-11

- Scope: provider item responses require nonblank, unique identities before
  entering media state.
- Hostile boundary: blank and duplicate/equivocated IDs fail closed before
  response order can select substituted content.
- Focused gate: `cargo test -p mde-jellyfin models::tests::duplicate_or_blank_item_identity_fails_closed_during_response_admission -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 11,678,840 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 98 filtered out.
- Remaining boundary: live provider browse/playback and package proof remain.
