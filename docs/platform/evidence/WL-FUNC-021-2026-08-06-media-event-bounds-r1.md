# WL-FUNC-021 evidence — bounded Media Player event handoff (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The authoritative `mde-media-core::Player` event handoff is now bounded to 256
pending events. Repeated `PositionChanged` events coalesce to the newest
position; a critical state, track, end, or error event evicts stale position
data first and then uses oldest-entry eviction as the final bounded fallback.
The player remains the sole transport/event authority and no UI-side queue was
introduced.

Changed file:

- `crates/desktop/mde-media-core/src/player.rs`

## Farm verification

All heavy verification ran on BigBoy `.130` in isolated slot
`media-core-r1`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-core-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-media-core \
  --all-features -- --nocapture
result: 235 unit tests, 1 real-mpv fixture, and 1 doctest passed; 0 failed

ssh mm@172.20.0.130 \
  'rustfmt --edition 2021 --check \
   crates/desktop/mde-media-core/src/player.rs'
result: pass
```

The hostile regression is
`player::tests::pending_events_coalesce_positions_and_bound_stalled_surfaces`.
It proves newest-position replacement, the exact 256-item cap, and critical
error retention after stale-position eviction. Local `git diff --check` passed.

## Runtime and remaining proof

The real-mpv fixture proves a nonblank decoded frame through the existing core
path, but does not claim live provider, output-device, casting, or Dell/seat-15
acceptance. Live Jellyfin credentials/server playback, target handoff/DLNA,
GUI-worker migration, direct DRM, and full release evidence remain open.

## Source hash at capture

```text
992287370a49d70a2e205ec19425ca9b1bf4140bf54c0b757b986edea7532c59  crates/desktop/mde-media-core/src/player.rs
```
