# WL-FUNC-011 V2 staging restart evidence — 2026-08-11

- Scope: each V2 transfer retry claims a unique create-new staging inode using
  a bounded sequence.
- Hostile boundary: stale same-attempt/PID residue cannot permanently block
  dirty-restart recovery. Pre-existing residue is preserved rather than deleting
  an inode that may be live or hostile.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::transfers::v2::tests::stale_same_attempt_staging_inode_cannot_block_restart_retry -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,841 filtered out.
- Remaining boundary: all typed executors and live cross-node transfer proof remain.
