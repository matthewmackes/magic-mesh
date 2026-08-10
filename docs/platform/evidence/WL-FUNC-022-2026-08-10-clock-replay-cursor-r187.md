# WL-FUNC-022 — Clock replay cursor evidence

- Date: 2026-08-10
- Farm host: `172.20.0.50`
- Farm slot: `func022-clock-replay-cursor-r187`
- Gate: `cargo test -p mackesd --lib workers::clock::tests::duplicate_clock_replay_cannot_regress_or_clear_action_cursor -- --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4723 filtered out`

Duplicate Clock request replays now preserve the durable action cursor when a
stale caller supplies an older cursor or no cursor. The hostile regression
proves that replay cannot move the cursor backward or erase it, preventing
already-admitted Bus commands from being consumed again after recovery.

Live limits: no physical multi-seat Clock execution, suspend/resume, or
cross-node production Bus proof was performed.
