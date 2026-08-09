# WL-FUNC-021 mpv playlist continuation — r11 (2026-08-09)

## Scope

This checkpoint advances WL-FUNC-021 S2's real playback path. It is limited to
the `mde-media-core` event seam and a real-mpv fixture; it does not claim live
renderer, authenticated Chromecast, seat-audio, or handoff acceptance.

## Production correction

- `MpvEngine` now preserves mpv's `MPV_END_FILE_REASON_REDIRECT` as a distinct
  non-terminal `EndReason::Redirect` instead of collapsing it into `Stopped`.
- `Player` keeps the active load authoritative across a playlist redirect so
  the resolved entry can reach `FileLoaded` and continue decode.
- A stale `EndFile(Stopped)` from the file superseded by a replacement
  `loadfile` no longer cancels the replacement while it is `Loading`.
- No Chromecast discovery, dependency, synchronous GUI browse, or unauthenticated
  CastV2 behavior is included in this checkpoint.

## BigBoy verification

Host: `172.20.0.130` (BigBoy), with isolated `MCNF_BUILD_SLOT` values.

1. Focused state-machine regressions:
   `cargo test -p mde-media-core continuation_ -- --nocapture`
   — **PASS**, 2 passed, 0 failed, 259 filtered out.
2. Exact real-mpv continuation fixture:
   `cargo test -p mde-media-core --features mpv --test mpv_fixture_decode real_mpv_playlist_redirect_continues_into_decoded_media -- --exact --nocapture`
   — **PASS**, 1 passed, 0 failed, 1 filtered out. The local M3U resolved into
   the checked-in VP8/Opus clip and produced a nonblank decoded frame in 0.31 s.
3. Exact-file `rustfmt --check --edition 2021` over the four changed Rust files
   — **PASS**. Package-wide formatting was intentionally not claimed because an
   unrelated pre-existing `roaming.rs` formatting diff remains outside this lane.

## Remaining boundary

Physical PipeWire audibility, live renderer continuity, authenticated CastV2,
and two-seat owner handoff remain governed live-proof work. This farm result
proves real local playlist/replacement continuation only.
