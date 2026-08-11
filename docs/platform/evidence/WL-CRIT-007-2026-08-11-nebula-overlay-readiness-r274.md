# WL-CRIT-007 Nebula overlay readiness evidence — 2026-08-11

- Scope: Nebula Supervisor restart no longer treats a retained `overlay-ip`
  file as proof that the transport survived.
- Boundary: startup atomically replaces retained readiness with an empty sealed
  value before phasing. The current bundle remains pending until configuration
  is reloaded and `nebula.service` is observed active; only then is the current
  overlay IP republished. Invalidation or service verification failure remains
  fail-closed and retryable.
- BigBoy (`172.20.0.130`) slot 1 exact regression
  `restart_invalidates_retained_overlay_until_nebula_is_verified_active`: PASS
  — 1 passed, 0 failed, 4,822 filtered.
- Scoped formatting and `git diff --check` passed.
- Remaining proof: installed restart/peer-return must show downstream bind
  consumers stay unready until the live overlay is active.
