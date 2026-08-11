# WL-FUNC-011 actor-log authenticity admission — 2026-08-11

- Scope: durable collaboration actor logs admit only current-schema envelopes with valid signatures.
- Hostile boundary: unsigned, invalid-signature, and future-schema events are rejected both on append and restart load.
- Focused gate: `cargo test -p mde-collab-core log::tests::unsigned_or_future_schema_event_cannot_enter_the_durable_actor_log -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 11,355,292 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 113 filtered out.
- Remaining boundary: live multi-peer signed offline replay proof remains.
