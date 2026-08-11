# WL-CRIT-007 missed-wake daemon restart — 2026-08-11

- Scope: Host State startup now includes a retained durable `Sleeping` intent
  in its return reconciliation. If mackesd restarts after missing logind's wake
  edge, NetworkManager stability produces one monotonic durable/Bus `Returned`
  event instead of leaving the peer permanently marked asleep.
- Production path: installed Host State worker → logind lifecycle monitor →
  durable availability intent → NetworkManager stability → health publication.
- Farm: focused patch gate on BigBoy `172.20.0.130`, slot `1`; subsequent
  integrated mackesd App/Workload gates compiled the complete shared tree on
  `.90`.
- Focused gate: `cargo test -p mackesd --lib workers::host_state::tests::startup_reconciles_sleep_when_daemon_missed_the_wake_signal -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,785 filtered out.
- Live boundary: physically suspend/resume one selected workstation while
  restarting mackesd across wake, then confirm one Returned event and peer.
