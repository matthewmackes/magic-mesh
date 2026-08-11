# WL-UX-009 evidence — persistent accent scheme at the frame boundary

- Date: 2026-08-10
- Scope: shared `mde-egui` Quazar style only
- Checkpoint: `set_color_scheme_and_accent` retains the selected Light/AutoSync3
  scheme after `egui::Context::run`, so the direct-DRM post-frame token remapper
  cannot silently fall back to Dark.
- Implementation: `crates/shared/mde-egui/src/style.rs`
- Regression: `accent_scheme_update_survives_the_egui_frame_boundary`
- Farm: `172.20.0.50`, slot `ux009-accent-scheme-boundary-r195`
- Result: `.50` passed `1 passed; 0 failed; 0 ignored; 0 measured; 290 filtered out`
  with `MCNF_BUILD_SLOT=ux009-accent-scheme-boundary-r195`.
- Live limits: no direct-DRM capture or physical-seat Light/AutoSync3 review was
  performed in this checkpoint.
