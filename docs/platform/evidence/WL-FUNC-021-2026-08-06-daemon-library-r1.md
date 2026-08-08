# WL-FUNC-021 — daemon-owned Library projection (2026-08-06)

## Implemented slice

`mde-music-egui` now treats a retained `MusicWorkspaceSnapshotV1` as the
Library authority. When the snapshot is present, Library renders bounded typed
`LibraryCollection` rows and does not read or paint the legacy Airsonic album
store. The standalone Airsonic worker remains an explicit compatibility path
only when no daemon snapshot has arrived yet.

Rows with playable kinds (`music`, `episode`, `chapter`, `audiobook`) publish a
validated source-qualified `MusicActionRequestV1` play request through the
authenticated shell writer. Album, artist, playlist, podcast, and radio rows
are visibly browse-only until the daemon exposes a matching typed operation;
they cannot accidentally emit an unsupported play request. Rows are capped at
`MAX_COLLECTION_ITEMS` and the UI reports an honest unavailable/empty state
when no retained collection exists.

## Hostile regression coverage

`daemon_library_prefers_typed_collections_over_legacy_album_store` constructs a
daemon `Songs` collection while also populating the old Airsonic album state.
The rendered frame contains the daemon row and does not contain the legacy
album; the selected daemon row emits the exact source-qualified typed play
request. The existing Home shelf test continues to cover source-variant
selection and cached playback metadata.

## Farm evidence

- Host `.50`, slot `music-daemon-library-r2`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-daemon-library-r2 ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  — **44 passed, 0 failed** (including binary/doc-test targets with no
  failures).
- Host `.90`, slot `music-daemon-library-fmt-r2`:
  `rustfmt --edition 2021 --check crates/desktop/mde-music-egui/src/app.rs`
  — **passed**.
- `git diff --check -- crates/desktop/mde-music-egui/src/app.rs` — **passed**.
- Source SHA-256: `a3ca2ff128a47638aaf04da2c95ec2b8f15f9153202dde1d4194c6dc7949f6e4`.

## Remaining boundary

Search still has a direct Airsonic worker request path because the retained
daemon search page does not yet have a shell read/request seam. Album detail
and live playback remain separately gated by real provider/audio evidence;
this slice does not infer those from headless rendering.
