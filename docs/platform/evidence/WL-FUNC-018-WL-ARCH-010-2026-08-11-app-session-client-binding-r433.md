# WL-FUNC-018 / WL-ARCH-010 App session client binding — 2026-08-11

- Scope: durable App-VM lifecycle handoff retains its initiating client across daemon restart.
- Hostile boundary: a second client cannot replay the session into another Workload operation or `OpenApp` handoff.
- Focused gate: `cargo test -p mackesd workers::cloud::verbs::app::tests::daemon_restart_cannot_rebind_app_session_to_substituted_client_peer -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 17,411,868 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,879 filtered out.
- Remaining boundary: replay an installed App-VM session from another seat after Cloud/session-broker restart.
