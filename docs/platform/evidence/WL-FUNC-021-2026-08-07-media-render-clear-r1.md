# WL-FUNC-021 current Media renderer gate (2026-08-07)

The current renderer slice clears the cached video texture whenever a new
title is in the real `Loading` state. This prevents a prior title's frame from
remaining visible beneath the new title while decoding is slow or fails.

Farm gates:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-egui-render-clear-r1
cargo test -p mde-media-egui --locked -- --nocapture
110 passed; 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-core-mpv-render-clear-r1
cargo test -p mde-media-core --features mpv --locked -- --nocapture
257 unit tests + 1 real-mpv fixture + 1 doctest passed; 0 failed
```

The Media UI test `loading_a_new_title_clears_the_previous_video_texture`
exercises the stale-frame boundary. The mpv fixture proves a nonblank decoded
frame and initialized audio route. Live physical renderer/PipeWire visual
acceptance remains separate and open.
