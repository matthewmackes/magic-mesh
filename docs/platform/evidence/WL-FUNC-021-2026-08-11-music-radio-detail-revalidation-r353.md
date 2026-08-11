# WL-FUNC-021 Music radio detail revalidation — 2026-08-11

- Scope: radio actions revalidate retained detail rows against the latest daemon
  catalog snapshot before publishing stream intent.
- Hostile boundary: withdrawn, changed, or conflicting station identities fail
  closed without emitting stale playback authority.
- Focused gate: `cargo test -p mde-music-egui app::tests::withdrawn_radio_detail_cannot_publish_its_stale_stream_identity -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 12.45 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 73 filtered out.
- Remaining boundary: live radio catalog mutation and installed-workspace proof remain.
