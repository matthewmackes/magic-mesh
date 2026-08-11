# WL-FUNC-021 Music current-catalog equivocation — 2026-08-11

- Scope: the current composite content identity maps to one consistent daemon
  catalog row before replacing UI playback metadata.
- Hostile boundary: conflicting rows reject the newer projection and retain the
  last-known-good title/artwork instead of selecting by source order.
- Focused gate: `cargo test -p mde-music-egui model::tests::equivocated_current_catalog_identity_cannot_replace_last_good_playback_projection -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 12.45 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 73 filtered out.
- Remaining boundary: live catalog replacement and installed-workspace proof remain.
