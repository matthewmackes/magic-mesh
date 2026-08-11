# WL-FUNC-020 future guest-inventory admission — 2026-08-11

- Scope: Cuttlefish guest inventory freshness is validated against the host clock before it can authorize Android readiness.
- Hostile boundary: a far-future observation with zero reported age cannot manufacture indefinitely fresh launch/VDI authority.
- Focused gate: `cargo test -p mackesd workers::cloud::verbs::android::cuttlefish_guest::tests::future_inventory_observation_cannot_invent_guest_readiness -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 1, admitted with 15,878,308 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,856 filtered out.
- Remaining boundary: live guest-agent clock skew and restart/reconnect proof remain.
