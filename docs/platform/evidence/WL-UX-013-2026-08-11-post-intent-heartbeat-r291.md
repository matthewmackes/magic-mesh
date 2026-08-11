# WL-UX-013 post-intent heartbeat evidence — 2026-08-11

- Scope: heartbeat evidence observed after expected-absence intent supersedes
  that intent, even when a stale sleep row is replayed after restart.
- Hostile boundary: the stale expected-absence generation cannot mask a later
  heartbeat outage or keep the node classified as intentionally absent.
- Focused gate: `cargo test -p mackesd workers::health_reconciler::tests::post_intent_heartbeat_prevents_stale_sleep_replay_from_masking_outage -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,829 filtered out.
- Remaining boundary: complete expected-state publisher, live transition, and
  three-seat history/recovery acceptance remain open.
