# WL-FUNC-021 Music server generation revocation — 2026-08-11

- Scope: explicit selection and approved failover stop playback owned by the
  previous server before publishing replacement server authority.
- Hostile boundary: loaded-track and progress state are cleared and `Stopped` is
  emitted, preventing stale audio from surviving the generation change.
- Focused gate: `cargo test -p mde-music-egui worker::tests::server_generation_change_revokes_old_playback_before_new_authority -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 15.5 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 70 filtered out.
- Remaining boundary: live provider failover and installed-workspace proof remain.
