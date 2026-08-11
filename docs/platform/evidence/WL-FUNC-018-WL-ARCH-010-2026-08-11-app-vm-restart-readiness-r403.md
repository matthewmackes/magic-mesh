# WL-FUNC-018 / WL-ARCH-010 App-VM restart readiness — 2026-08-11

- Scope: reconstructed session history cannot stand in for current App-VM guest readiness after the broker restarts.
- Hostile boundary: recovered `Connected` becomes `Reconnecting` while retaining its generation watermark; stale pre-restart evidence remains refused and only a forward generation restores `Connected`.
- Focused gate: `cargo test -p mackesd workers::session_broker::tests::recovered_app_vm_cannot_republish_historical_connected_readiness -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 10,479,664 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,869 filtered out.
- Remaining boundary: live serving-node restart with a connected App VM and next-generation guest recovery proof remain.
