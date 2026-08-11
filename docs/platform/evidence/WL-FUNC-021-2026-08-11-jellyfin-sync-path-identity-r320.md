# WL-FUNC-021 Jellyfin sync path identity — 2026-08-11

- Scope: played-state synchronization encodes user and item identities as exact
  single URL path segments.
- Hostile boundary: separators, queries, fragments, and pre-encoded escapes
  cannot alias another user's or item's played/unplayed endpoint.
- Focused gate: `cargo test -p mde-jellyfin sync::tests::played_state_identity_cannot_escape_or_alias_path_segments -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 11,137,744 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 93 filtered out.
- Remaining boundary: live sync/outage and package proof remain.
