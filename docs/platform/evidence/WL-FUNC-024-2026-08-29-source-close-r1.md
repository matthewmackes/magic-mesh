# WL-FUNC-024 source close — 2026-08-29

Classification: source/cargo close. Live two-seat audio, chirp correlation,
LiveKit SFU, and PSTN remain `WL-TEST-003` after a testing Beta.

Tree: `5f9685408` plus the voice-admin persist compile fix (dirty).
`production_admitted: false`. No dest invented. No seat mutation.

## Why this closes

S1–S6 are in-tree: typed `MediaSessionV1` contracts, the mackesd
call-media P2P/SFU/SIP planes, Calls UI mute/DTMF/camera binds that
refuse recorded intent, and typed reconnecting/failed states. S2
validation is the loopback/chirp fixture. Live two-seat leftover was
already moved to `WL-TEST-003`.

## Farm (dirty tree; one crate, not re-run)

| command | job | node | ended | result |
|---|---|---|---|---|
| `cargo test -p mde-collab-egui` | `bdec5d40433e` | `.50` d1 | 2026-08-29T11:43:16Z | 203 passed, 0 failed |

Prior honesty note: `WL-FUNC-024-2026-08-26-source-media-plane-honesty-r1.md`.
