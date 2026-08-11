# WL-ARCH-008 session restart readiness evidence — 2026-08-11

- Scope: the session broker no longer treats a durable historical `Active`
  transition as proof that a Browser VM presentation survived a daemon restart.
- Boundary: after the first successful durable action drain, every recovered
  `Active` row is demoted to `Disconnected` before shared-plane convergence.
  This replaces stale etcd/file-backed ready state instead of renewing it. Only
  a subsequent authorized forward `Active` action can restore readiness.
- Focused farm gate:
  - Host: `172.20.0.50`, slot 1.
  - Command: `cargo test -p mackesd --features async-services workers::session_broker::tests::restart_recovery_refuses_to_republish_historical_active_session_as_ready -- --exact --nocapture`
  - Result: PASS — 1 passed, 0 failed, 4,813 filtered.
- `git diff --check` passed.
- Remaining live boundary: restart/peer-return hardware proof must show that the
  presentation stays disconnected until the client authenticates a reconnect.
