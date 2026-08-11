# WL-FUNC-021 Jellyfin transcode authority — 2026-08-11

- Scope: server-provided transcode paths remain bound to the selected server,
  item, media source, session, and token.
- Hostile boundary: absolute, protocol-relative, cross-item, fragmented,
  malformed, and oversized redirects are rejected in favor of a locally bound URL.
- Focused gate: `cargo test -p mde-jellyfin playback::tests::hostile_transcode_url_cannot_escape_selected_server_or_item_authority -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 9.7 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 94 filtered out.
- Remaining boundary: live Jellyfin playback/outage and package proof remain.
