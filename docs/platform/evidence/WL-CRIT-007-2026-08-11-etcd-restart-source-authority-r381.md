# WL-CRIT-007 etcd restart source authority — 2026-08-11

- Scope: configured etcd remains the authoritative health source across restart and outage.
- Hostile boundary: an unavailable etcd lease snapshot cannot fall through to stale filesystem heartbeat data and fabricate healthy readiness.
- Focused gate: `cargo test -p mackesd workers::health_reconciler::tests::configured_etcd_restart_failure_cannot_substitute_fresh_filesystem_health -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 11,044,744 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,854 filtered out.
- Remaining boundary: installed-node etcd outage/recovery and fleet convergence proof remain.
