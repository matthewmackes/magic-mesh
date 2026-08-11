# WL-FUNC-018 Front Door serving route admission — 2026-08-11

- Scope: Front Door emits App launch requests only for typed safe serving-node identities.
- Hostile boundary: path-like or multiline serving nodes fail before Bus payload construction.
- Focused gate: `cargo test -p mde-shell-egui front_door::tests::peer_app_launch_wire_rejects_unsafe_serving_route_before_bus -- --exact --nocapture`.
- Farm: clean rerun on `172.20.0.50`, slot 2, admitted with 13,708,316 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 1,560 filtered out; two earlier colliding runs were terminated and not claimed.
- Remaining boundary: inject a malformed real discovery route and verify no `action/apps/launch` message is emitted.
