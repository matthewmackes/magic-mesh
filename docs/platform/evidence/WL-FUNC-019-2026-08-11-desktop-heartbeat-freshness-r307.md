# WL-FUNC-019 desktop heartbeat freshness evidence — 2026-08-11

- Scope: desktop-seat and Workload resource projection requires a current peer
  heartbeat; zero or future-dated observations are stale negative authority.
- Hostile boundary: plausible reachable rows cannot borrow identity/readiness
  from a future peer observation.
- Focused gate: `cargo test -p mackesd --lib --features async-services workers::desktop_sources::tests::future_peer_heartbeat_cannot_authorize_seat_or_workload_resources -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 3.
- Result: **PASS**, 1 passed, 0 failed, 4,841 filtered out.
- Remaining boundary: live authenticated desktop discovery/action/render and
  recovery acceptance remain open.
