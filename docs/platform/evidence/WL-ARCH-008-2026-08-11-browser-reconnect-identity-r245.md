# WL-ARCH-008 Browser reconnect identity — 2026-08-11

- Scope: the installed session broker treats an exact Browser VM open as a
  reconnect replay, preserving active state, timestamps, serving/client peers,
  and transport. The same stable session ID cannot retarget its VM route or
  RDP/Sunshine profile, and a closed Browser session requires a new ID.
- Production path: `Surface::Browser` → typed Workload Start/Resume → Browser
  VDI `SessionRequest::Open` → installed session broker → roaming roster.
- Farm: focused Browser patch gate on BigBoy `172.20.0.130`; the subsequent
  integrated mackesd App/Workload gates compiled the complete shared tree on
  `.90`.
- Focused gate: `cargo test -p mackesd --lib workers::session_broker::tests::browser_reconnect_replay_preserves_live_route_and_transport -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,785 filtered out.
