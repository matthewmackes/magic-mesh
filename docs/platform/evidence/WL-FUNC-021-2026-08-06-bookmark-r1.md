# WL-FUNC-021 — Music typed bookmarks (2026-08-06)

## Scope

The Music workspace action contract now admits `bookmark` and
`bookmark_delete`. Create requires a bounded finite `position_ms` and an
admitted Episode, Chapter, or Audiobook identity; delete requires the same
admitted identity. The daemon routes both mutations through the retained
source variant and matching provider client, and the Airsonic adapter calls
typed Subsonic `createBookmark` and `deleteBookmark` endpoints. No provider
URL, credential, or free-form command reaches the Bus action boundary.

This is a write-side bookmark slice. Provider bookmark listing, snapshot shelf
projection, capability negotiation, and live podcast/audiobook acceptance are
still remaining work.

## Farm verification

- `.50`, slot `music-bookmark-fmt-r1`: `cargo fmt -p mde-musicd -- --check`
  passed.
- `.90`, slot `music-bookmark-focused-r1`: the hostile
  `bus_responder::tests::typed_bookmark_uses_the_selected_admitted_provider`
  regression passed 1/1; the selected provider succeeded and an unadmitted
  identity was refused.
- BigBoy `.130`, slot `music-bookmark-full-r1`: `cargo test -p mde-musicd
  --lib` passed 162/162 with 0 failures.
- BigBoy `.130`, slot `music-bookmark-full-r1`: `cargo test -p mde-musicd
  --doc` passed 0/0 with 0 failures.
- Local `git diff --check` passed after the farm wave.

## Source integrity

```text
310c3ba08cf461a2c0402ffb94a261c93464d87fa45b0e5e6977c9324f9063dd  crates/services/mde-musicd/src/bus_responder.rs
016f7bbce4b300214c249f148bceb26a44ed354cfdf102b3f0bff0bc1e8ab4e1  crates/services/mde-musicd/src/airsonic.rs
f14daf1ba02ce3f7802af0daa7cc5b7c3973009f9197bbfb188400331c448369  crates/services/mde-musicd/src/domain.rs
```

This is fixture-backed typed provider-admission evidence. It is not live
provider capability or shelf proof, audible playback, target/DLNA handoff,
GUI-worker removal, or Dell runtime acceptance.
