# WL-FUNC-024 video-track integration evidence — 2026-08-20

## Deliverable

The mackesd P2P and LiveKit-SFU media workers now preserve `CallKind::Video`
as a bounded audio-plus-camera offer. The retained `MediaSessionV1` document
and both offer/answer descriptions carry `[audio, video]`; audio-only calls
retain their existing `[audio]` contract. The renderer and SIP/provider paths
were not changed.

## Focused verification

Formatting was checked for the touched media worker; the repository already
has unrelated formatting drift in `crates/mesh/mackesd/src/onboard/remote_push.rs`.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=0 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib call_media::tests
Result: 6 passed, 0 failed
```

The first admitted attempt on `172.20.0.50` failed before test execution with
`No space left on device`; it was not treated as a source failure. The retry
used the same dirty source and focused command on the admitted `.90` lane and
passed all six media-worker fixtures, including
`video_call_carries_audio_and_camera_tracks_through_p2p_signaling`.

## Boundary and blockers

This slice proves typed video-track propagation, not camera frame capture or
video-frame correlation. A real WebRTC media stack, camera-device proof, and
qualification-seat evidence remain required for the full WL-FUNC-024 outcome
and are not claimed here. The requested `crates/desktop/mde-voice-hud/**`
directory does not exist in this checkout; the live voice-HUD crate is under
`crates/services/mde-voice-hud`, which was intentionally left untouched.
