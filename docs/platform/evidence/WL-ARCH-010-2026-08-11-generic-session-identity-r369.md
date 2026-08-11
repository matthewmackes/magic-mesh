# WL-ARCH-010 Generic session identity binding — 2026-08-11

- Scope: an admitted generic session ID remains bound to its original workload
  and peer route across exact retries and lifecycle updates.
- Hostile boundary: conflicting workload/peer reuse and closed-session
  resurrection fail closed instead of retargeting authority.
- Focused gate: `cargo test -p mackesd workers::session_broker::tests::generic_session_id_cannot_retarget_an_active_workload_or_route -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 8,826,856 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,852 filtered out.
- Remaining boundary: live multi-node session-store and presentation proof remain.
