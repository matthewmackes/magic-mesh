# WL-FUNC-021 — daemon Home shelf projection (2026-08-06)

Status: implementation and focused farm verification complete; the epic remains
`Remaining` because live provider/audio, network-loss, seat handoff, DLNA,
GUI-worker removal, and release acceptance evidence remain open.

## Change

Music Home now prefers the daemon-owned `MusicWorkspaceSnapshotV1.shelves`
projection when a retained snapshot contains shelves. Each shelf renders the
bounded typed `CatalogItem` rows, preserves source/cache truth, and selects the
daemon's deterministic first reachable-or-cached `SourceVariant` for playback.
The resulting play request is validated and sent through the existing
authenticated shell `action/music/workspace` publisher. If no admitted variant
exists, the UI reports an unavailable state; if no shell authority is present,
it reports that playback is unavailable. The legacy Airsonic worker remains
only as the explicit compatibility fallback when no daemon shelf is available,
so this slice does not invent a second daemon state store or claim full worker
removal.

## Verification

- Farm `.50`, slot `music-daemon-home-r1`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-daemon-home-r1
  ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  passed 43 tests, 0 failed.
- The new regression renders a typed daemon shelf and verifies a cached,
  source-qualified variant becomes a validated `play` request.
- Farm `.90`, slot `music-daemon-home-fmt-r1`:
  `rustfmt --edition 2021 --check
  crates/desktop/mde-music-egui/src/app.rs` passed.
- Local `git diff --check` passed.
- Source SHA-256:
  `crates/desktop/mde-music-egui/src/app.rs`
  `509d18d5b695acf3c60056cfd8c2cbf3d4e01b8f2d21af727ba1decafb4ff617`.
