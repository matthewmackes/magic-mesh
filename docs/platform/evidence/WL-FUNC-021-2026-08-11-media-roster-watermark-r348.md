# WL-FUNC-021 Media roster watermark — 2026-08-11

- Scope: the Media UI model retains the newest admitted roster publication
  watermark across explicit withdrawal.
- Hostile boundary: stale snapshots cannot restore a gateway revoked by newer
  state; corrected-forward publication restores authority.
- Focused gate: `cargo test -p mde-media-egui model::tests::stale_mesh_roster_cannot_restore_revoked_gateway_authority -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 8,789,488 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 113 filtered out.
- Remaining boundary: live roster withdrawal/recovery and installed-UI proof remain.
