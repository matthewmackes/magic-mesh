# WL-FUNC-018 Front Door declaration equivocation evidence — 2026-08-11

- Scope: peer App declarations are grouped by `(node_id, app_id)`. Exact
  duplicates collapse, while conflicting catalog revision, capabilities,
  profile, lifecycle state, or source plane suppresses that launch identity.
- Hostile boundary: input order cannot select one conflicting declaration, and
  unrelated valid apps remain visible and launchable.
- Focused gate: `cargo test -p mde-shell-egui front_door::tests::conflicting_peer_app_declarations_never_become_launch_targets -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 1.
- Result: **PASS**, 1 passed, 0 failed, 1,555 filtered out.
- Remaining boundary: governed image boot, live launch/readiness/presentation,
  cleanup, and release proof remain open.
