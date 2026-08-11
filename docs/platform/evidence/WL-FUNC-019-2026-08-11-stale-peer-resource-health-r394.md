# WL-FUNC-019 stale peer resource health — 2026-08-11

- Scope: universal-resource peer cards must not project retained health as current after their membership observation expires.
- Hostile boundary: a stale peer heartbeat carrying `healthy` is retained as a visible stale card, but its health becomes `Stale` with an explicit stale-membership failure.
- Focused gate: `cargo test -p mackesd workers::service_aggregator::resource_adapters::tests::stale_peer_heartbeat_cannot_fabricate_current_resource_health -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 2, admitted with 15,017,160 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,864 filtered out.
- Remaining boundary: live authenticated Windows discovery/login/render and publisher deployment proof remain.
