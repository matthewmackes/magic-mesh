# WL-FUNC-021 state file no-follow — 2026-08-11

- Scope: coordination-state reads use no-follow, require regular files, and use
  nonblocking descriptors so special files cannot stall startup.
- Hostile boundary: a symlink cannot restore forged playback authority after
  restart.
- Focused gate: `cargo test -p mde-musicd state::tests::symlinked_state_cannot_restore_substituted_playback_authority_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 10.1 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 258 filtered out.
- Remaining boundary: live state replacement/restart and installed-daemon proof remain.
