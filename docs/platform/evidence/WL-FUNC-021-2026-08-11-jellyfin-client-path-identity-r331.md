# WL-FUNC-021 Jellyfin client path identity — 2026-08-11

- Scope: remote user, series, and artwork item identities are encoded as exact
  single URL path segments.
- Hostile boundary: separators, queries, fragments, and pre-encoded escapes
  cannot select a substituted endpoint.
- Focused gate: `cargo test -p mde-jellyfin client::tests::remote_ids_cannot_escape_their_request_path_segment -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 8,674,248 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 95 filtered out.
- Remaining boundary: live remote browse/artwork and package proof remain.
