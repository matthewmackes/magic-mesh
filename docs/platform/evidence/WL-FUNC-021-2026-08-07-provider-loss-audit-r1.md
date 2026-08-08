# WL-FUNC-021 — provider-loss/recovery acceptance audit (2026-08-07)

## Finding

The existing `install-helpers/verify-music-network-loss.sh` proved only the
transport boundary: clean FIN versus a mid-stream TCP reset, followed by the
policy refusal of a byte-zero fallback. It did not exercise the bounded
same-provider recovery contract now used for an admitted Subsonic stream.

That gap is now covered by the helper's disposable loopback fixture. After the
reset has emitted audio, the client requests the same logical stream with
`/rest/stream?id=song-7&timeOffset=1`, verifies a complete non-silent recovery
body, accounts for the audible prefix plus resumed bytes, and asserts that no
`/fallback` request occurred. The fixture remains loopback-only and is cleaned
up on exit.

## Verification

- `bash -n install-helpers/verify-music-network-loss.sh` — passed.
- `install-helpers/verify-music-network-loss.sh --self-test` — passed locally.
- Focused farm gate on `.50`, slot
  `music-network-loss-helper-r1` — passed. The result reported a
  `ConnectionResetError`, exactly one `timeOffset=1` recovery request,
  `recovery_audio_bytes=9600`, `logical_audio_bytes=19200`, and
  `fallback_requests=0`.
- Read-only live observation on seat `172.20.0.15`, bounded to three samples:
  all samples were `service=active provider=ok catalog=ok state=ok`; the helper
  returned its expected refusal because no natural provider loss occurred.

## Acceptance boundary

The helper now proves a deterministic provider/client recovery witness, not a
live `mde-musicd` transition. Live provider-loss/recovery, audible continuity,
decoder behavior on the installed seat, and audio-hardware acceptance remain
unproven because no safe natural outage was observed and the probe does not
interrupt a provider or mutate seat/network state.
