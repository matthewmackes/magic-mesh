# WL-FUNC-017 status weather coordinate identity — 2026-08-11

- Scope: status-bar current conditions require exact latitude/longitude identity
  with the effective location, in addition to matching host and generation.
- Hostile boundary: mismatched coordinates fail closed to `Weather unavailable`
  before rendering or action. A corrected-forward exact location restores the
  weather projection; stale-but-valid locations remain supported.
- Focused gate: `cargo test -p mde-shell-egui status_bar::tests::weather_projection_is_generation_scoped_fresh_or_explicitly_stale -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 1,555 filtered out.
- Remaining boundary: installed status-bar captures and live Maps/weather proof remain.
