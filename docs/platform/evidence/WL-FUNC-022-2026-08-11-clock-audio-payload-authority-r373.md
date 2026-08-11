# WL-FUNC-022 Clock audio payload authority — 2026-08-11

- Scope: an active occurrence remains bound to its admitted audio payload across same-generation retries.
- Hostile boundary: a conflicting retry cannot substitute source authority and receive a successful acknowledgement.
- Focused gate: `cargo test -p mde-musicd clock_audio::tests::active_occurrence_cannot_acknowledge_a_conflicting_audio_payload -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 1, admitted with 14,035,836 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 261 filtered out.
- Remaining boundary: live physical-audio retry/fallback proof remains.
