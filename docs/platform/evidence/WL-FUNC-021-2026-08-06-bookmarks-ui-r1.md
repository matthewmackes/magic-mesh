# WL-FUNC-021 — Music bookmark shelf projection (2026-08-06)

## Implemented slice

The Music Home surface now consumes the daemon-owned retained
`MusicWorkspaceSnapshotV1.bookmarks` projection and renders a bounded `Resume`
shelf. Each row retains the provider-qualified typed identity behind the
snapshot, shows provider title/creator/parent metadata, and displays the finite
resume position plus a bounded progress bar when duration is known.

This is deliberately read-only in the existing UI worker: it does not invent a
bookmark store, provider call, or click-to-play path for episode/chapter/
audiobook rows. Those rows remain honest resume metadata until the typed daemon
playback action is exposed by this surface.

Changed file:

```text
crates/desktop/mde-music-egui/src/app.rs
```

## Farm verification

All compile/test/format work used explicit farm hosts and isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-bookmark-ui-focused-r1 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-music-egui daemon_bookmarks_render_as_typed_resume_metadata
result: 1 passed, 0 failed; 37 filtered out

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-bookmark-ui-full-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-music-egui
result: 38 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-bookmark-ui-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check
result: pass
```

The exact farm scratch workspaces were removed after completion. Local
`git diff --check` passed. This fixture-backed UI result does not claim live
provider bookmark access, live podcast/audiobook playback, or Dell/seat
acceptance.

## Source hash

```text
f131f4ecc502b96b3a8ea4c6f0e162a616e97425809f870b5a062bf629c547e0  crates/desktop/mde-music-egui/src/app.rs
```

Working-tree base revision: `e52322ec` (changes are intentionally
uncommitted).
