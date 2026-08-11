# WL-FUNC-016 V2 checkpoint ordering — 2026-08-11

- Scope: the installed clipboard-sync worker stops a V2 Bus drain when a
  terminal row cannot be durably checkpointed. A later rich clipboard envelope
  can no longer materialize past the unacknowledged row; retry resumes from the
  same cursor boundary.
- Farm: BigBoy `172.20.0.130`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clipboard_sync::tests::v2_checkpoint_failure_stops_before_later_materialization -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,795 filtered out.
