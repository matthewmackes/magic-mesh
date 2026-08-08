# WL-FUNC-021 — daemon queue projection in the Music Now Playing rail (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
full Bus parity, GUI-worker removal, live provider/audio, network-loss, seat,
DLNA, and release acceptance remain open.

## Goal

Make the Music Now Playing rail consume the daemon's retained typed queue
projection instead of presenting a placeholder or maintaining a second GUI
queue authority.

## Implementation

- `crates/desktop/mde-music-egui/src/app.rs` adds a bounded read-only queue
  renderer that consumes `MusicWorkspaceSnapshotV1.queue`, marks the daemon's
  current `ContentRef`, preserves source identity and content kind, and reports
  additional entries beyond the UI display bound honestly.
- The renderer emits no queue mutations; reorder, remove, and playback remain
  typed daemon actions. Missing snapshots and empty queues are explicit states,
  not fabricated rows.
- A CPU render regression covers a typed source-qualified queue entry and the
  current marker. The standalone workspace's existing bookmark render remains
  green, and the Construct shell mount compiles and renders the same surface.

## Farm verification

- `.50`, slot `music-queue-rail-ui-r1`: `cargo test -p mde-music-egui` — `42
  passed, 0 failed`, including the queue projection/current-marker regression.
- `.90`, slot `music-queue-rail-fmt-r1`: package-scoped `cargo fmt -p
  mde-music-egui -- --check` passed.
- `.90`, slot `music-queue-rail-shell-r1`: focused
  `cargo test -p mde-shell-egui --no-default-features
  shell_mounts_and_renders_the_media_surface` — `1 passed, 0 failed`; the
  shell test binary compiled with the changed Music surface.
- BigBoy `.130` is down/unreachable; no BigBoy result is claimed.

## Remaining proof

This proves the retained queue projection and embedded render path, not live
queue mutation or playback. Full catalog/action Bus parity, queue mutation UI,
live provider decode, cache/network-loss playback, seat handoff, DLNA, Dell,
and release evidence remain required before the Music epic or drain goal can
close.
