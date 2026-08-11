# WL-FUNC-021 cast discovery equivocation — 2026-08-11

- Scope: exact duplicate renderer observations collapse while conflicting
  protocol/endpoint bindings suppress the shared renderer identity.
- Hostile boundary: discovery source order cannot choose which physical device
  receives control; unrelated valid renderers remain available.
- Focused gate: `cargo test -p mde-media-core cast::tests::equivocated_discovery_identity_cannot_select_a_renderer_by_source_order -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 9,269,344 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 268 filtered out.
- Remaining boundary: live physical renderer discovery/control remains.
