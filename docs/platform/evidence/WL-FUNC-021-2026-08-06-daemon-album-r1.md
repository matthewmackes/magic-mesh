# WL-FUNC-021 — daemon-owned album detail (2026-08-06)

## Implemented slice

Embedded `mde-music-egui` now opens retained daemon album items from Home,
Library, and Search. When a `MusicWorkspaceSnapshotV1` is present, the album
detail view derives its bounded track list only from the typed `Music`
collection, reports a clear unavailable state when the retained window has no
matching tracks, and publishes source-qualified typed `play` actions for
selected tracks. It does not issue the legacy Airsonic `LoadAlbum` request in
this path. The standalone client remains the explicit compatibility path when
no daemon snapshot exists.

## Hostile regression coverage

`daemon_album_detail_uses_typed_song_collection_and_typed_play` opens a typed
album beside a typed song collection, renders the track without the legacy
loading state, and asserts the selected track emits the exact source-qualified
`MusicActionRequestV1` play request.

## Farm evidence

- Host `.50`, slot `music-daemon-album-r1`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-daemon-album-r1 ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  — **46 passed, 0 failed** across library, binary, and doctest targets.
- Host `.50`, slot `music-daemon-album-fmt-r2`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-daemon-album-fmt-r2 ./install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check`
  — **passed**.
- `git diff --check -- crates/desktop/mde-music-egui/src/app.rs` — **passed**.
- Music UI source SHA-256: `d2c2672cd6a8fe1de87ba2f68ade8178d0b3dbc5befa48392a025a437f2b75db`.

## Remaining boundary

The standalone Airsonic worker fallback, real mpv/PipeWire audio-video
evidence, managed download execution, live provider/cast/handoff fixtures, and
package/seat acceptance remain open. Headless UI coverage does not infer live
playback or renderer availability.
