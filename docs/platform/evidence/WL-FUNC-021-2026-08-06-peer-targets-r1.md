# WL-FUNC-021 peer-target discovery evidence — 2026-08-06

## Scope

The daemon workspace snapshot now projects the bounded peer-heartbeat roster as
typed `mesh_seat` playback targets. Fresh idle peers are actionable. A fresh
peer currently owning playback and a stale heartbeat remain visible with an
explicit unavailable reason. The projection skips the local peer, sorts target
identity deterministically, and truncates the combined local/mesh list to the
contract bound. No renderer is manufactured from a configured URL.

The existing typed Music UI handoff path can therefore offer real retained
peer identities while keeping unavailable targets honest. The transfer daemon
path now also rejects arbitrary, stale, or currently-owning peer identities
before checking local engine readiness, then owns durable takeover-intent
admission.

## Focused evidence

- `crates/services/mde-musicd/src/bus_responder.rs` adds the bounded
  `playback_targets` projection and
  `workspace_targets_project_fresh_idle_and_refused_peer_heartbeats` hostile
  test.
- Farm test:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-peer-targets-r1 ./install-helpers/xcp-build.sh cargo test -p mde-musicd`
  — 169 passed, 0 failed; the final formatted-source rerun includes fresh,
  owning, stale, and non-admitted transfer refusal coverage.
- Farm format gate:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-peer-targets-guard-fmt-r1 ./install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check`
  — passed.
- Local `git diff --check` — passed.

## Source hashes

- `crates/services/mde-musicd/src/bus_responder.rs`
  — `1b31a23fe64c6cbc71b46240fcf97edc01d44f6b98c6c95a37d68141b428ab23`
- `crates/desktop/mde-music-egui/src/app.rs`
  — `e890af70327030610f36d834820f75ccd1d5db2b3a7ee2d8b3b6a2186fea3596`

## Limitations / remaining acceptance

This proves retained peer projection and refusal behavior, not live mesh
network reachability or target-side playback. Live owner-yield/resume,
network loss, audio/video continuity, DLNA discovery/control, provider/cache
behavior, package/RPM proof, and Dell/seat-15 acceptance remain open.
