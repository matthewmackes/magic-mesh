# WL-FUNC-021 playlist symlink and atomicity — 2026-08-11

- Scope: playlist saves use unique same-directory staging, file synchronization,
  and atomic replacement; loads admit regular files only.
- Hostile boundary: symlink substitution cannot redirect a save or supply a
  playlist from a different inode after restart.
- Focused gate: `cargo test -p mde-media-core playlist::tests::symlink_cannot_substitute_or_redirect_playlist_persistence -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 9.2 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 263 filtered out.
- Remaining boundary: live playlist continuation and installed-player proof remain.
