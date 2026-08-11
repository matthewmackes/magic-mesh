# WL-FUNC-021 roaming lease publication — 2026-08-11

- Scope: fresh login and resume arm playback only after durable lease publication
  and immediate confirmation of the exact seat and generation.
- Hostile boundary: an obstructed lease path leaves the prior seat authoritative;
  the claimant pauses, clears authority, and returns `LeaseUnavailable`.
- Focused gate: `cargo test -p mde-media-core roaming::tests::failed_lease_publication_cannot_resume_on_non_owner_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 265 filtered out.
- Remaining boundary: live two-seat audible handoff and installed-player proof remain.
