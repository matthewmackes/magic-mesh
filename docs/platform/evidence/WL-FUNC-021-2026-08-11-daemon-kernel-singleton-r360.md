# WL-FUNC-021 daemon kernel singleton — 2026-08-11

- Scope: `mde-musicd` claims node-wide kernel-lifetime ownership before starting
  its Bus responder.
- Hostile boundary: a duplicate daemon cannot consume actions or publish playback
  authority; exit/crash releases ownership without stale lockfiles.
- Focused gate: `cargo test -p mde-musicd --bin mde-musicd tests::duplicate_daemon_cannot_publish_and_released_owner_recovers_immediately -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 10.3 GiB free.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: installed service restart/singleton proof remains.
