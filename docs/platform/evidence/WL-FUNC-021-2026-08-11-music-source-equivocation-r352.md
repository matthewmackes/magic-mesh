# WL-FUNC-021 Music source equivocation — 2026-08-11

- Scope: workspace projections require nonblank canonical unique Music source
  identities before deriving reachability or capabilities.
- Hostile boundary: provider ordering cannot select contradictory declarations
  for one identity and invent daemon authority.
- Focused gate: `cargo test -p mde-music-egui workspace_reader::tests::equivocated_source_identity_cannot_invent_daemon_reachability -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 15.2 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 72 filtered out.
- Remaining boundary: live provider projection/recovery and installed-workspace proof remain.
