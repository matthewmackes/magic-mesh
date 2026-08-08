# WL-FUNC-021 live provider/audio probe — 2026-08-06

This is a bounded non-production live probe on Basement seat 15
(`172.20.0.15`). It uses the installed review payload and does not install,
reboot, or mutate provider data.

## Provider and playback result

- The operator-owned Airsonic credential file exists with mode `0600`; its
  contents were never printed.
- `mde-musicd ping --retry 0` reached the real provider at API version
  `1.15.0` and returned success.
- Authenticated read-only catalog requests returned album `1701`, “The Speed
  of Metal”, with 10 songs. A follow-up album read returned song `23427`,
  “Blood Moon Prelude”, `66` seconds, `flac`.
- A bounded `timeout 15s mde-musicd play 23427` run emitted
  `mde-musicd: playing 1 track(s)` with no decode or HTTP error. While it was
  running, PipeWire reported active 2-channel 44.1 kHz sink inputs and
  `pw-cli` exposed `alsa_playback.mde-musicd` stream nodes. The CLI process then
  ended after the intentional bounded probe window.
- The installed `mde-musicd.service` remained active with `NRestarts=0`.
- The canonical Bus root `/run/mde-bus` returned `get-state` with
  `audio_available:true`, `needs_airsonic:false`, and a parseable idle state;
  `list-albums` returned the same live album roster. A deliberately unsigned
  `action/music/enqueue` request was answered with
  `authorization refused: no armed token supplied`, and the queue remained
  empty, proving the mutation boundary is active rather than silently bypassed.

This proves a real provider catalog read, stream fetch/decode startup, and
PipeWire routing on the enrolled workstation. It does not claim a complete
66-second audible capture, nonblank DRM frame, network-loss resume, or target
handoff.

## Authorization blocker

The live daemon journal reports `music mutation authorization unavailable`.
The user unit has no `LoadCredential*` entry. A read-only systemd user-manager
smoke test attempting to load the existing root-owned encrypted
`cloud-arm-key` returned `243/CREDENTIALS`. The gate therefore remains
fail-closed; no credential workaround or provider mutation was attempted.
The correct remaining work is a governed per-seat Music authorization delivery
path that does not expose the root cloud-arm mint key to an untrusted user
process.

## Commands

```text
ssh mm@172.20.0.15 'mde-musicd ping --retry 0'
ssh mm@172.20.0.15 'mde-musicd play 23427'  # bounded with timeout 15s
ssh mm@172.20.0.15 'pactl list sink-inputs short; pw-cli ls Node'
ssh mm@172.20.0.15 'mde-bus request action/music/get-state --bus-root /run/mde-bus --json'
ssh mm@172.20.0.15 'mde-bus request action/music/list-albums --bus-root /run/mde-bus --json'
ssh mm@172.20.0.15 'systemd-run --user --wait --pipe \
  -p LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key ...'
```
