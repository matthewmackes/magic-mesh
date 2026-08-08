# WL-FUNC-021 projection validation checkpoint (2026-08-06)

## Scope

The Music UI now treats the daemon's retained `MusicWorkspaceSnapshotV1` as a
hostile typed-process boundary. A newer snapshot must validate before it can
replace the last known-good projection; stale revisions are ignored, invalid
revisions/content are reported as an actionable UI error, and playback,
position, and volume are projected only from an accepted snapshot. The daemon
contract now rejects revision zero with the stable `invalid_revision` code.

## Farm verification

- `.50` (`MCNF_BUILD_SLOT=music-projection-validation-20260806-r2`):
  `cargo test --locked -p mde-music-egui 'daemon_snapshot_' -- --nocapture`
  passed **4/4**.
- `.90` (`MCNF_BUILD_SLOT=music-domain-revision-20260806-r2`):
  `cargo test --locked -p mde-musicd 'snapshot_validation_' -- --nocapture`
  passed **1/1** through `install-helpers/xcp-build.sh`.

The focused tests cover transport/volume projection, retention of the prior
projection after hostile storage content, and the domain contract's revision
zero rejection. No live provider, renderer, or installed-seat mutation was
performed.

## Files

- `crates/services/mde-musicd/src/domain.rs`
- `crates/desktop/mde-music-egui/src/model.rs`

## Limitations

This closes the source-level projection boundary only. WL-FUNC-021 still needs
live network-loss recovery, owner-yield/resume handoff, rendered acceptance,
and current installed-seat package proof.
