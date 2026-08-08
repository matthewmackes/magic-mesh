# WL-FUNC-021 target handoff evidence — 2026-08-06

## Scope

The Music surface now renders a bounded daemon-retained playback-target list.
Available `mesh_seat` targets expose a `Send` control that publishes the typed
`MusicActionRequestV1` `transfer` action with the target peer. Unavailable
targets show the daemon's reason. Local and DLNA targets remain visible but
are explicitly browse-only until their typed adapters exist; the UI does not
silently substitute a target or call a backend directly.

The daemon transfer path validates the target, rejects a local target, requires
an active local engine, rejects playback owned elsewhere, and persists the
takeover request through the existing state authority.

## Focused evidence

- `crates/desktop/mde-music-egui/src/app.rs` adds
  `transfer_playback_to_target`, bounded target rendering, and the hostile
  `mesh_target_handoff_emits_typed_transfer_request` test.
- Farm test: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-target-ui-r3 ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  — 48 passed, 0 failed; the transfer test asserted `action == "transfer"`
  and `target_peer == "peer:seat-15"`.
- Farm format gate:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-target-ui-fmt-r2 ./install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check`
  — passed.
- Local `git diff --check` — passed.

## Source hashes

- `crates/desktop/mde-music-egui/src/app.rs`
  — `e890af70327030610f36d834820f75ccd1d5db2b3a7ee2d8b3b6a2186fea3596`
- `crates/services/mde-musicd/src/bus_responder.rs`
  — `cf23c0ec2246dda6a6262aabd4848427dbb3f9f1a6c7488613127aee16b99d6a`

## Limitations / remaining acceptance

This is typed request and refusal coverage, not live seat acceptance. A live
peer/seat fixture is still required to prove owner yield, target resume,
network loss, audio/video continuity, and recovery. DLNA/local control adapters,
live provider/cache behavior, RPM/package proof, and Dell/seat-15 visual/audio
acceptance remain open under WL-FUNC-021.
