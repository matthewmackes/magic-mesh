# WL-CRIT-007 Nebula restart revalidation — 2026-08-11

- Scope: restart retracts retained overlay-IP readiness until configuration reload and `nebula.service` active verification succeed.
- Hostile boundary: failed verification cannot republish old overlay authority and leaves the bundle pending for retry.
- Focused gate: `cargo test -p mackesd workers::nebula_supervisor::tests::restart_invalidates_retained_overlay_until_nebula_is_verified_active -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 23,029,388 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,858 filtered out.
- Remaining boundary: installed restart must prove expected `nebula1` identity and peer dataplane traffic.
