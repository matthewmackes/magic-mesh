# WL-FUNC-021 — daemon-owned Search request/projection (2026-08-06)

## Implemented slice

Embedded `mde-music-egui` now accepts a shell-owned read-only browse publisher.
When that publisher is installed, the debounced Search route emits only the
bounded `{"query": ...}` body to the `search` browse verb and renders the
daemon-retained `MusicWorkspaceSnapshotV1.search` page. The surface no longer
issues a direct Airsonic worker search in the embedded shell. The standalone
client leaves the worker fallback intact until it has a shell browse boundary.

The daemon snapshot remains the only rendered search authority in the embedded
path. A missing/stale page stays in a bounded loading state rather than showing
old provider rows; daemon catalog items use the same source-qualified typed
play path as Home and Library, while non-playable kinds remain browse-only.

The shell installs the browse writer beside the authenticated workspace mutation
writer. The writer accepts only `search` and publishes to the canonical
`action/music/search` topic; all provider I/O and retained projection work stays
inside `mde-musicd`.

## Hostile regression coverage

`embedded_search_uses_daemon_browse_publisher_instead_of_worker` installs the
publisher on a worker-less Music app, requests `blue hour`, and asserts the
exact `search` verb/body plus `Fetch::Loading` state. The full UI test also
continues to cover daemon Home/Library projection, typed playback, queue, and
bookmark rendering.

## Farm evidence

- Host `.50`, slot `music-daemon-search-r1`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-daemon-search-r1 ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  — **45 passed, 0 failed**.
- Host `.50`, slot `music-daemon-search-fmt-r2`:
  `rustfmt --edition 2021 --check crates/desktop/mde-music-egui/src/app.rs`
  — **passed**.
- Host `.90`, slot `music-daemon-search-shell-r1`:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-daemon-search-shell-r1 ./install-helpers/xcp-build.sh cargo test -p mde-shell-egui`
  — package compiled; **1,441 passed, 10 failed** in pre-existing unrelated
  shell catalog/console/taskbar/IAC/snapshot fixtures. No failure named the new
  Music browse publisher or Music UI test.
- `git diff --check` for the touched Music and shell source files — **passed**.
- Music UI source SHA-256: `c7f124f4e42602639f478c2e64a5d56f61fa4652d5a459bb572e24d0ec093ecd`.
- Shell integration source SHA-256: `c47fa483f01c2729cd864bc7d8879317f321d610ec1f8787d98cd395ae87b7f0`.

## Remaining boundary

The full shell gate is not green because of the ten unrelated pre-existing
failures named above. Live provider search, real media decode/audio/video, and
package/seat acceptance remain open; this fixture evidence does not infer them.
