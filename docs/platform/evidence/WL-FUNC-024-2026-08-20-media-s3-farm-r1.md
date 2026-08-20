# WL-FUNC-024 media S1–S6 farm evidence — 2026-08-20

## Deliverable

Source revision: `41080a75c822a019252a06778f1474f7751532c1` plus the dirty
media-plane changes listed below.

The shared media contracts now admit bounded `MediaSessionV1` and
`SfuElectionV1` documents. `mackesd` contains the WebRTC P2P plane and the
elected LiveKit SFU plane. Both planes bind seat audio honestly, route mute
and DTMF through the live leg, publish typed session state, and refuse to
claim `Connected` without advancing frames. Group calls publish an election
document and enter `Reconnecting` when SFU health is withdrawn.

## Farm verification

All commands were run against the dirty worktree through the governed farm
helper; no local cargo build was used.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-types --lib media::tests
Result: 10 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib call_media::tests
Result: 5 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib collab_media::tests
Result: 16 passed, 0 failed
```

The focused fixtures cover hostile contract admission, two-seat chirp
correlation, mute and DTMF, device absence, permission denial, group election,
SFU loss, and typed reconnecting without a false connected state. Live provider
credentials and installed-seat acceptance remain owned by `WL-TEST-002`.

