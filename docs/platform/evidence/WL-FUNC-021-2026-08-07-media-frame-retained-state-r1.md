# WL-FUNC-021 — media frame retained-state coalescing

Date: 2026-08-07

## Finding

The Media app already polls the retained `state/media/sources` Bus record on a
coarse 60-frame cadence, but every identical decoded roster was assigned back
to the controller. That needless replacement can re-run source projection work
in synchronized seats.

## Change

`BusMediaSources` in
`crates/desktop/mde-media-egui/src/app.rs` now retains the last applied roster
and skips controller assignment when the decoded record is equal. Changed
rosters still apply normally. The per-frame `MediaController::pump()` remains
unconditional, preserving the playback clock and live transport controls.

## Verification

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=media-frame-retained-state-r2 \
install-helpers/xcp-build.sh cargo test --locked -p mde-media-egui --lib \
bus_media_sources_skips_unchanged_roster_reapplication -- --nocapture
```

Result: **1 passed, 0 failed, 108 filtered out** on build host
`172.20.0.90`. The regression exercises initial application, an identical
record that must preserve the retained allocation, and a changed record that
must reach the controller.

Live Dell/seat acceptance was not performed; this evidence is farm-only.
