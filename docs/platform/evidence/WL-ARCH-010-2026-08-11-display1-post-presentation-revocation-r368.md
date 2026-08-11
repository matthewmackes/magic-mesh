# WL-ARCH-010 Display1 post-presentation revocation — 2026-08-11

- Scope: acknowledged presentation authority is revalidated without consuming
  queued input.
- Hostile boundary: shell disconnect, lease expiry, or peer failure revokes
  retained frame readiness and advances the input epoch.
- Focused gate: `cargo test -p mackesd display1_broker::tests::post_presentation_disconnect_revokes_retained_frame_authority -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 15.8 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,852 filtered out.
- Remaining boundary: live Display1 disconnect/reconnect and installed-shell proof remain.
