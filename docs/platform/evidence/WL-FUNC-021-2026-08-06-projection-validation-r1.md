# WL-FUNC-021 daemon Music projection validation — 2026-08-06

`mde-music-egui` now validates each daemon-owned
`MusicWorkspaceSnapshotV1` before replacing the retained UI projection. An
invalid newer snapshot is refused, the last valid revision remains visible,
and no local worker fallback is created. Valid snapshots continue to project
playback position and volume through the typed state.

Verification:

- The new hostile regression covers invalid storage content and preservation
  of the prior valid revision; `git diff --check` passed.
- Farm `.50`, slot `music-func021-projection-invalid-20260806-r1`, attempted
  `cargo test -p mde-music-egui daemon_snapshot_ -- --nocapture` but the host
  reached `ENOSPC` during compilation. No passing test result is claimed.
- Source SHA-256:
  `c1dd6e9da3753ee3f0183202e9751701393579bb4aee109601a17a142034f4ef`.

This is source-level authority hardening only. Live daemon projection,
rendered Music acceptance, provider/audio recovery, handoff/cast, package, and
seat proof remain open. Dell was not modified.
