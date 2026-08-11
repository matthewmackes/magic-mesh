# WL-CRIT-007 host-state Bus inode recovery — 2026-08-11

- Scope: Host State activation binds the exact Bus SQLite device/inode identity.
- Hostile boundary: same-path Bus replacement re-primes action and snapshot
  tails, skips retained replacement actions, and blocks host mutation until a
  fresh post-replacement seat snapshot arrives.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::host_state::tests::same_path_bus_replacement_reestablishes_snapshot_freshness_barrier -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,842 filtered out.
- Remaining boundary: installed sleep/restart and fleet recovery acceptance remain.
