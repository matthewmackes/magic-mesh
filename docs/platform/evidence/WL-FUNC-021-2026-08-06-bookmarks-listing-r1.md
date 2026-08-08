# WL-FUNC-021 — Music bookmark listing and retained shelf (2026-08-06)

## Goal slice

Complete the read side of the typed Music bookmark path. The daemon now reads
the provider's `getBookmarks` response through the admitted Airsonic/Subsonic
client, bounds and normalizes the nested bookmark rows, maps only the closed
Music media kinds (`music`, `episode`/`podcast`, `chapter`, and `audiobook`),
and retains source-qualified bookmark rows in `MusicWorkspaceSnapshotV1`.
Each retained row carries its finite millisecond resume position so a player
surface can resume it without creating a second bookmark authority. Unknown
provider media types are omitted from the typed shelf rather than presented as
playable content. Older snapshots remain readable because the new field has a
serde default.

The provider read is also exposed as the bounded `list-bookmarks` browse verb,
is included in read-only multi-source fan-out, and participates in the same
catalog persistence bounds and selected-source admission checks used by the
existing Music mutations.

## Farm verification

All build/test work used isolated farm slots:

- `.90`, `MCNF_BUILD_SLOT=music-bookmarks-r1`: `cargo test -p mde-musicd
  bookmark` — 3 passed, 0 failed. This covers nested provider parsing,
  typed projection/admission, and selected-provider bookmark mutation.
- `.90`, `MCNF_BUILD_SLOT=music-bookmarks-full-fallback-r1`: `cargo test -p
  mde-musicd` — 164 passed, 0 failed; doctests — 0 passed, 0 failed.
- `.90`, `MCNF_BUILD_SLOT=music-bookmarks-egui-r1`: `cargo test -p
  mde-music-egui workspace_reader` — 2 passed, 0 failed.
- `.50`, `MCNF_BUILD_SLOT=music-bookmarks-fmt-r3`: `cargo fmt -p mde-musicd
  -- --check` — passed.
- The required BigBoy `.130` full-suite retry was attempted with
  `MCNF_BUILD_SLOT=music-bookmarks-full-r1`, but the host returned `No route
  to host`; no BigBoy result is claimed for this slice.
- Local `git diff --check` and the canonical governance lints were rerun after
  the evidence update.

## Honest remaining boundary

This is fixture-backed typed provider and retained-state evidence. It does not
claim provider capability negotiation, live podcast/audiobook credentials or
playback, GUI shelf rendering, live two-catalog outage behavior, target/DLNA
handoff, direct DRM, or Dell runtime acceptance. The bookmark mutation and
listing paths remain provider-adapter boundaries until live sources are
available.

The endpoint contract is the Subsonic `getBookmarks` read API:
[Subsonic API](https://subsonic.org/pages/api.jsp).

## Source integrity

```text
0b25f5c56130117f5b5a6e08bd419c141b54df5c96781d0aba3d77ca9ddd3105  crates/services/mde-musicd/src/airsonic.rs
2977e04838f76d3ee9c9276a29c15c0c2ca04a752f69b9ef578243fe74061125  crates/services/mde-musicd/src/domain.rs
4f7e2671cde7b6516a03a2f0fbc606a70054080e95d947e2c07369859b3a9d0c  crates/services/mde-musicd/src/bus_responder.rs
159cb3debb6d75fe1a86341016a1087a9d0d0f475f4625e019a5eb89240c7e51  crates/desktop/mde-music-egui/src/workspace_reader.rs
3b1da0fac147495de76a73783544e3fe1cfd2f0b2d8e083636554bcc04e2727e  docs/platform/WORKLIST.md
```

The worktree contains unrelated user changes; they were preserved and are not
part of this slice.
