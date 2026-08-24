# WL-FUNC-024 S1 collab-bus media board admission — 2026-08-24

## Correctness seam

`MediaSessionV1` already fail-closed untyped JSON on `state/calls/media/<session>`.
The collab-bus sidecar boards that the live leftover actually observes —
`state/collab/call-media-readiness` and `state/collab/call-media-verification` —
were still plain serde shapes decoded with `serde_json::from_str`. A hostile
publisher could claim `live_media_verified` with no frames, attach SDP/path/
secret fields, or advertise `adapter_ready` with only the local actor.

`CallMediaReadiness::from_json` / `CallMediaVerification::from_json` now own
bounded admission in `mde-collab-types`: size, duplicate keys,
`deny_unknown_fields`, actor/kind pairing, and the same frames-required honesty
lock as `MediaSessionStateV1::Connected`. Empty boards remain honest absence
(the Seat 15 leftover shape). This crate still does not mint a live call.

## Focused farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-types
Result: 101 passed, 0 failed
```

Admission: 27753444 KiB free on `.50` (required 8388608 KiB). No ENOSPC.

## Boundary and blockers

This closes typed admission for the collab-bus media boards only. Live two-seat
audio with objective chirp/tone correlation, group SFU, and PSTN remain the
`@leftover:{live-seat}` leftover after a current-revision unpublished candidate
is installed. Do not invent a dest. Do not flip `production_admitted`.
