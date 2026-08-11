# WL-FUNC-011 actor-log pathname identity — 2026-08-11

- Scope: each collaboration actor log is bound to the `(space, actor)` identity
  declared by its pathname.
- Hostile boundary: a mismatched envelope is rejected before append or file
  creation, and a misplaced durable row fails closed during restart replay.
- Focused gate: `cargo test -p mde-collab-core log::tests::actor_log_path_identity_rejects_misplaced_events_live_and_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 111 filtered out.
- Remaining boundary: complete signed Chat/Alerts migration and live release proof remain.
