# WL-FUNC-021 Music cache path identity — 2026-08-11

- Scope: exact encoded song/suffix identities map to bounded unique cache files;
  legacy lossy paths are admitted only when unambiguous.
- Hostile boundary: IDs that formerly collapsed under sanitization retain
  distinct same-sized offline audio across restart.
- Focused gate: `cargo test -p mde-musicd cache::tests::sanitized_id_alias_cannot_substitute_same_sized_offline_audio_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 8,789,408 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 251 filtered out.
- Remaining boundary: live replicated-cache and installed-daemon proof remain.
