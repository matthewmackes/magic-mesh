# WL-FUNC-021 universal codec capability — 2026-08-11

- Scope: the universal `ffmpeg-free` baseline advertises only codecs guaranteed
  without the optional RPM Fusion codec swap.
- Hostile boundary: H.264/AVC and H.265/HEVC cannot leak into baseline direct-play
  capability and suppress required Jellyfin transcoding.
- Focused gate: `cargo test -p mde-media-core capabilities::tests::optional_codec_pack_cannot_leak_into_the_universal_baseline -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 11,269,016 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 279 filtered out.
- Remaining boundary: installed-package codec and live direct-play/transcode proof remain.
