# WL-FUNC-022 — Clock audio replay cursor evidence

- Date: 2026-08-10
- Farm host: `172.20.0.50`
- Farm slot: `func022-audio-replay-cursor-monotonic-r195`
- Gate: `cargo test -p mackesd --lib workers::clock::tests::stale_clock_action_and_audio_cursors_cannot_regress -- --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4728 filtered out`

The Clock worker now advances action and Music audio-status replay cursors only
when the candidate Bus ULID is newer than the durable in-memory boundary. A
stale or reordered audio status therefore cannot move the replay boundary
backward and cause an already-consumed acknowledgement to be replayed after
recovery. The hostile regression covers an older action cursor, a forward
action cursor, and an older audio-status cursor after initialization.

Live limits: no physical multi-seat Clock execution, suspend/resume, native
audio-device, or cross-node production Bus proof was performed.
