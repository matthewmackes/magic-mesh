# WL-FUNC-022 Clock occurrence payload binding — 2026-08-11

- Scope: each active occurrence generation remains bound to its original Start
  audio and volume payload.
- Hostile boundary: conflicting payloads fail with `occurrence_payload_conflict`
  and cannot be acknowledged as successfully playing.
- Focused gate: `cargo test -p mde-musicd clock_audio::tests::active_occurrence_cannot_acknowledge_a_conflicting_audio_payload -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 1, admitted with 14,035,836 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 261 filtered out.
- Remaining boundary: live ringing/audio replacement and installed-seat proof remain.
