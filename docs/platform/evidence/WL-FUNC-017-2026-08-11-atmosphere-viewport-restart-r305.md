# WL-FUNC-017 atmospheric viewport restart evidence — 2026-08-11

- Scope: retained fallback geometry must exactly match the deterministic
  location-derived viewport, preserving its source identity across restart.
- Hostile boundary: forged fallback coordinates and retained Maps-action rows
  with generations incapable of superseding the fallback are rejected before
  atmospheric publication.
- Focused gate: `cargo test -p mackesd --lib --features async-services workers::weather_atmosphere::tests::restart_rejects_retained_viewport_without_source_identity -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,841 filtered out.
- Remaining boundary: live NOAA/Maps publication and release acceptance remain.
