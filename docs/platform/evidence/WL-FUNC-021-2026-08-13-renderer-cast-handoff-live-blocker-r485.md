# WL-FUNC-021 renderer/cast/handoff live blocker

Date: 2026-08-13

## Scope audited

The Music UI/service scope under `crates/desktop/mde-music-egui` already has
typed, testable paths for the remaining renderer/audio/cast/handoff boundary:

- transport controls publish through the authenticated daemon action path;
- playback targets are admitted only when the retained target identity still
  matches the current daemon projection;
- only available `mesh_seat` targets expose typed playback handoff;
- unavailable, withdrawn, non-mesh, and unauthenticated targets fail closed;
- renderer/audio generation and cast URL admission are covered by existing
  focused farm evidence.

Adding another adapter or local fallback in this scope would duplicate an
existing authority or fabricate a live target, so no source change was made.

## External blocker

The remaining acceptance is physical/live-only: an admitted physical renderer
or Chromecast receiver, a real daemon-owned renderer/audio path, and a second
approved seat/mesh owner are required to prove cast control, audible continuity,
renderer recovery, and two-seat handoff. The repository evidence explicitly
records that no physical renderer, usable Chromecast path, receiver unit, or
second admitted peer was available. Fixtures and loopback tests cannot prove
that boundary.

## Evidence consulted

- `docs/platform/WORKLIST.md` WL-FUNC-021 cast-loopback, two-seat handoff,
  cast-runtime-audit, renderer-recovery, and live-boundary entries.
- `crates/desktop/mde-music-egui/src/app.rs` typed target admission,
  authenticated transfer publication, and daemon-owned transport controls.
- Existing bounded cast/handoff/renderer evidence cited by the worklist.

## Gates

No cargo gate was run: this audit made no source change, and a retest would not
produce the missing physical/live proof.

## Remaining acceptance

Provide an approved physical renderer or Chromecast receiver and a second
admitted seat/mesh owner, then capture real audio/video, cast control/seek,
renderer-loss recovery, and exact-once two-seat handoff after release.
