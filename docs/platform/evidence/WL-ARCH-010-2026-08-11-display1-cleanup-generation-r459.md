# WL-ARCH-010 Display1 cleanup generation — 2026-08-11

- Scope: restart cleanup removes only the Display1 socket generation it owns.
- Hostile boundary: concurrent replacement cannot be unlinked by a stale broker cleanup path.
- Focused gate: `cargo test -p mackesd display1_broker::tests::restart_cleanup_cannot_unlink_a_concurrent_newer_broker_socket -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live direct-DRM replacement proof.
