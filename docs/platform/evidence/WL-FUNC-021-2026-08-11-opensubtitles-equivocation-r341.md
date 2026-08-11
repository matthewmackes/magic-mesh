# WL-FUNC-021 OpenSubtitles equivocation — 2026-08-11

- Scope: exact duplicate file bindings collapse while conflicting metadata for
  one `file_id` suppresses that identity.
- Hostile boundary: provider response order cannot select an equivocated
  subtitle file; unrelated valid results remain available.
- Focused gate: `cargo test -p mde-media-core opensubtitles::tests::equivocated_file_identity_cannot_be_selected_by_provider_order -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 11,835,980 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 276 filtered out.
- Remaining boundary: live provider search/download and installed-player proof remain.
