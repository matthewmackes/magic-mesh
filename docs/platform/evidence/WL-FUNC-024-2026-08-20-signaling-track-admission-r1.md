# WL-FUNC-024 signaling track-admission evidence — 2026-08-20

## Correctness seam

The media contract validated each offer/answer's fingerprint and actor
direction, but did not require the description's track set to match the
session's offered tracks. A validly fingerprinted audio-only answer could
therefore be accepted for a video session. The mackesd P2P and SFU workers also
treated that mismatch as negotiable instead of publishing typed
`InvalidSignaling`.

`MediaSessionV1::validate` now requires both descriptions to carry exactly the
session's offered track set. The P2P and SFU workers reject a mismatched
session id or track set and publish `MediaSessionStateV1::Failed` with
`InvalidSignaling`; the existing audio-plus-camera propagation path is
unchanged.

## Focused farm verification

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib call_media::tests
Result: 6 passed, 0 failed
```

The gate compiled the changed `mde-collab-types` dependency and passed the
P2P/SFU media fixtures, including video-track signaling, frame honesty,
device-unavailable states, election, and SFU recovery.

## Boundary and blockers

This closes typed signaling admission only. The workspace still has no real
WebRTC media stack or camera frame capture, and no installed-seat or governed
PSTN provider proof. The live `mde-voice-hud` crate remains under
`crates/services/mde-voice-hud`; it was not changed because this seam is
transport admission, not SIP account driving.
