# WL-FUNC-021 — signed live radio and release 8 fleet rollout (2026-08-08)

## Outcome

C-SPAN Radio now traverses the production typed path on Dell: retained
Airsonic radio identity, root-shell Ed25519 authorization, daemon admission,
direct HTTP(S) stream decoding, PipeWire output, and typed Stop cleanup. The
same signed Play/Stop probe also passed on seat 15. This closes the typed live
radio playback boundary; it does not close the wider Music/Media epic.

Two independent defects caused the visible Play control to fail:

1. `mde-musicd` verified `music_auth`, then deserialized the still-signed JSON
   into `MusicActionRequestV1`, whose `deny_unknown_fields` correctly rejected
   the wire-only authorization field as `invalid_request`. The responder now
   removes only the already-verified `music_auth` field before domain decoding;
   every other unknown field remains rejected.
2. The user-owned daemon stored consumed nonces below root-only
   `/var/lib/mackesd`. Release 7 moved that replay ledger into the daemon's
   user-owned state directory. The shared secret helper also now selects a
   reachable configured etcd member and retries KV requests across configured
   endpoints instead of feeding an empty first-member response to Python.

## Farm and package evidence

- BigBoy `.130`, slot `music-radio-auth-r8`: focused signed-envelope regression
  passed 1/1; the test also proves unrelated unknown fields remain rejected.
- Prior release-7 focused gates in the same correction chain: typed radio 3/3
  and authorization/replay-ledger 4/4 passed on BigBoy.
- `.90`, slot `music-secret-failover-r8`: offline endpoint-failover regression
  passed. Local `bash -n` and ShellCheck (excluding pre-existing SC2015 notices)
  passed.
- Native Fedora 44 BigBoy builder `.131` produced
  `magic-mesh-12.1.6-8.x86_64.rpm`, 87,604,723 bytes, SHA-256
  `070d4b8d00c0eba424ddb2ee7adb648b3201de530b3d245bfaa9da465dc129cf`.
- Payload/size gates passed. Runtime sonames are Fedora 44-native:
  `libavcodec.so.62`, `libswresample.so.6`, `libswscale.so.9`, and
  `libmpv.so.2`.
- The temporary F44 builder was halted after the cut and normal BigBoy farm VM
  `.130` was restored.

## Live proof

All five physical seats report `magic-mesh-12.1.6-8.x86_64` with `mackesd`,
`mde-musicd`, and `mde-shell-egui` active: T480, Eagle, seat 15, Dell, and
Surface. Each current age identity was idempotently registered; only
`music/action-ed25519-seed` was resealed to the current recipient set. Each seat
then materialized a host-encrypted Music credential and public verification
key. No private seed was printed or copied between hosts.

Dell's bounded `verify-music-live-radio.sh --play-probe` passed package/service
provenance, provider ping, exact C-SPAN catalog lookup, retained typed
`ContentRef` admission, signed Play, two consecutive active engine samples,
signed Stop, and cleanup. A concurrent default-sink monitor captured 2,621,440
bytes of 48 kHz stereo s16le PCM: 287,035 non-zero samples, peak 20,092, and RMS
1,677.73. Temporary PCM and capability files were removed.

Seat 15 independently passed the same signed catalog/admission/Play/two-sample/
Stop probe on release 8. This proves the resealed credential path works beyond
Dell.

## Honest remaining boundary

- No human speaker-loudness judgment was made; the Dell PipeWire sink capture
  is decoded-output proof.
- T480, Eagle, and Surface have release/service/credential proof, but did not
  receive a mutating radio probe in this record.
- Album/artist/podcast completeness, named unavailable records, provider-loss
  continuity, physical DLNA/Chromecast rendering, peer handoff, and five-seat
  synchronized CPU/NWS recovery remain open under WL-FUNC-021.
