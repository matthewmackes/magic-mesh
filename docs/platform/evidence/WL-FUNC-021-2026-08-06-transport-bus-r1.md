# WL-FUNC-021 — shell Music transport Bus admission (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
full GUI-worker migration, live provider/audio, network-loss, seat, DLNA, and
release acceptance are still open.

## Goal

Route shell Music transport intent through the existing authenticated daemon
workspace lane while keeping the standalone client honest until the complete
catalog and worker migration is finished.

## Implementation

- `crates/desktop/mde-music-egui/src/app.rs` now builds validated typed
  `MusicActionRequestV1` requests for `pause`, `resume`, `stop`, `seek`, and
  `set_volume` when the Construct shell publisher is installed.
- The shell publisher remains the existing root-authorized
  `action/music/workspace` Bus writer; the UI holds no armed token and no
  second executor or transport state.
- Standalone Music has no publisher, so the same controls explicitly fall back
  to the bounded legacy worker path. This is a compatibility boundary, not a
  claim that GUI-worker removal is complete.
- `crates/desktop/mde-music-egui/src/model.rs` now applies daemon playback
  `playing`, `position_ms`, and `volume_milli` projection when a newer
  workspace revision is accepted, so shell controls converge on daemon state
  instead of resetting the volume control every frame.
- The daemon already maps these typed actions to its sole transport authority:
  pause/stop retain best-effort final progress, seek requires a bounded
  position, and volume converts the bounded milli-unit field to the existing
  engine representation.

## Farm verification

- `.50`, slot `music-volume-projection-ui-r1`: `cargo test -p mde-music-egui`
  — `41 passed, 0 failed`, including the shell-publisher,
  standalone-fallback, and daemon-volume-projection regressions.
- `.90`, slot `music-transport-shell-r1`: focused
  `cargo test -p mde-shell-egui --no-default-features
  shell_mounts_and_renders_the_media_surface` — `1 passed, 0 failed` with the
  full shell crate compiled.
- `.50`, slot `music-transport-daemon-parser-r1`: daemon transport parser
  coverage — `3 passed, 0 failed` for verbs, seek forms, and volume forms.
- `.90`, slot `music-transport-daemon-scope-r1`: authenticated mutation scope
  coverage — `1 passed, 0 failed`, including pause/resume/stop/seek/volume
  transport scopes and read-only exclusions.
- `.90`, slot `music-volume-projection-fmt-r1`: package-scoped `cargo fmt -p
  mde-music-egui -- --check` passed after the volume projection update.
- BigBoy `.130` was unreachable/down for this slice; no BigBoy result is
  claimed.

## Remaining proof

This is fixture-backed farm evidence, not live playback. Full Bus parity and
removal of the GUI-owned provider/engine worker, live two-catalog playback,
network-loss cache playback, audible engine proof, target-seat handoff, DLNA,
Dell/seat acceptance, and release/RPM gates remain required before
WL-FUNC-021 or the broader drain goal can close.
