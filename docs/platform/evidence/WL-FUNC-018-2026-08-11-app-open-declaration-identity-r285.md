# WL-FUNC-018 App-open declaration identity evidence — 2026-08-11

- Scope: a live App VM session ID is an immutable authority-lifetime boundary.
  Reuse now requires the complete admitted `OpenApp` declaration to match,
  including catalog revision, capabilities, and resume intent as well as the
  VM, peers, app identity, and guest profile.
- Hostile boundary: conflicting replays fail atomically and preserve the
  existing lifecycle state and timestamps; an exact replay remains
  idempotent.
- Focused gate: `cargo test -p mackesd --features async-services workers::session_broker::tests::repeated_app_open_reuses_only_the_exact_bound_declaration -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,826 filtered out.
- Remaining boundary: governed image supply, live boot/readiness, presentation,
  cleanup, and release acceptance remain open.
