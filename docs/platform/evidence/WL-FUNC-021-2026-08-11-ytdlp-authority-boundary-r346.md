# WL-FUNC-021 yt-dlp authority boundary — 2026-08-11

- Scope: page URLs are credential-free HTTP(S) before subprocess launch and
  provider-returned media URLs pass the same authority admission.
- Hostile boundary: ambiguous/disguised authorities and local/non-network schemes
  cannot cross the yt-dlp boundary.
- Focused gate: `cargo test -p mde-media-core ytdlp::tests::ambiguous_authority_cannot_cross_the_ytdlp_boundary -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 11,239,320 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 279 filtered out.
- Remaining boundary: live provider extraction and installed-player proof remain.
