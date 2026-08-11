# WL-UX-013 health-modal privacy — 2026-08-11

- Scope: operator-visible Health modal text redacts secret-like material and local path data from hostile provider, node, summary, and action-impact fields before rendering.
- Hostile boundary: an otherwise valid health projection carrying credentials and filesystem paths cannot expose those values in the rendered UI.
- Focused gate: `cargo test -p mde-shell-egui health_modal::tests::hostile_health_projection_cannot_render_secret_or_path_material -- --exact --nocapture`.
- Farm: clean coordinator-only run on `172.20.0.170`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 1,561 filtered out.
- Remaining boundary: render and capture a hostile live Bus projection on direct-DRM hardware to prove the same privacy boundary end to end.
