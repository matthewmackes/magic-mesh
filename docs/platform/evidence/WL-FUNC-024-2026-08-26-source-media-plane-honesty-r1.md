# WL-FUNC-024 S5 source — media-plane honesty (no recorded intent) — r1

Date: 2026-08-26  
Classification: source honesty; **not** two-seat audio, **not** chirp-correlation
production proof, **not** LiveKit SFU, **not** PSTN, **not**
`production_admitted`  
Source worktree: `agent/drain-worklist-20260725`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

No dest invented. No PSTN credentials. No Sunshine. Seat 15, Dell, and Surface
Construct were not occupied (peer live workers hold those DRM seats).

## Correctness seam

The Calls UI still had recorded-intent paths that could look like media was
flowing:

- `set_call_muted` / `send_dtmf` emitted `SetCallMuted` / `SendDtmf` with no
  published [`MediaSessionV1`](crates/shared/mde-collab-types/src/media.rs).
- `reconcile_media_intent` kept camera/screen bits while signaling
  `CallParticipantState::Connected` even when `state/calls/media/<session>`
  was empty.
- mute/DTMF classified `Negotiating` / `Reconnecting` as live if
  `audio_bound` was set.
- collab-bus `CallMediaAdmission::AdapterReady` had no `claims_live_media`
  helper, so a caller could confuse signed-state admission with frames.

Those paths now bind only to existing typed topics:

- mute/DTMF emit only when `MediaSessionV1::binds_live_mute` /
  `binds_live_dtmf` (Connected + observed frames + the matching sender).
- camera/screen bits follow `MediaSessionV1::offered_live_track` only.
- roster copy is `· N connected` only with live frames; otherwise
  `· N in call · media unavailable` (or negotiating).
- `CallMediaReadiness::claims_live_media` is always false. AdapterReady
  cannot enable mute/DTMF. Verification `LiveMediaVerified` still does not
  substitute for the session bind.
- `PstnAgentDrive::claims_live_pstn` is always false. Ready is driveable,
  not a live SIP dialog.

## Focused farm verification

`.170` was `FULL(disk)` (7.4G free, 8G light headroom). Light crates went to
`.50` / `.90`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-types
Result: 102 passed, 0 failed
Admission: 30260280 KiB free on .50 (required 8388608 KiB).

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-voice-hud
Result: 65 passed, 0 failed
Admission: 45986496 KiB free on .90 (required 8388608 KiB).

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-egui
Result: 193 passed, 0 failed
Admission: 39793320 KiB free on .90 (required 8388608 KiB).
```

No ENOSPC. No workspace grind.

## Remaining live leftover (still required)

Live leftover stays open. Closing it needs two-seat audio with objective
chirp/tone correlation on current-revision seats after a current unpublished
candidate is installed. This unit did not SSH Seat 15, Dell, or Surface.

Blocker (unchanged from Surface 2026-08-25 dest-cut probe): collaboration
identity receipt `source_revision` is still `7e3474eeb` vs installed SHA
`4071ed295`. `collab` did not spawn; there is no `state/calls/media`. That is
FUNC-023 identity leftover, not a `calls.rs` gap. PSTN still depends on
FUNC-030 `gateway.toml`. Do not invent a dest. Do not flip
`production_admitted`.
