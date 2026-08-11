# WL-FUNC-021 media-source heartbeat freshness — 2026-08-11

- Scope: retained media-source reachability requires a nonzero heartbeat no
  later than the restart sample.
- Hostile boundary: zero and future-dated retained heartbeats cannot republish a
  source as reachable after daemon restart.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::media_sources::tests::retained_impossible_heartbeat_cannot_reauthorize_media_source_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,845 filtered out.
- Remaining boundary: live provider discovery/playback and package/seat proof remain.
