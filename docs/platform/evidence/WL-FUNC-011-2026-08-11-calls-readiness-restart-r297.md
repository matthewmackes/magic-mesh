# WL-FUNC-011 Calls readiness restart evidence — 2026-08-11

- Scope: missing or corrupt retained Calls readiness is negative authority on
  restart and tombstones prior media verification state.
- Hostile boundary: stale `LiveMediaVerified` proof cannot survive an unreadable
  readiness generation. Corrected-forward readiness can subsequently re-enter
  normal provider verification without preserving the stale claim.
- Focused gate: `cargo test -p mackesd --features async-services workers::collab_media::tests::corrupt_readiness_after_restart_revokes_stale_live_media_proof -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,836 filtered out.
- Remaining boundary: a production Calls provider, live media/control,
  consent/revocation, and release proof remain open.
