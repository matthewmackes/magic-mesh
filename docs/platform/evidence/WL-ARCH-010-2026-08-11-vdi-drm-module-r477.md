# WL-ARCH-010 DRM VDI module evidence — 2026-08-11

- Scope: the DRM-enabled shell VDI module exercises native Display1 activation,
  RDP/SPICE frame upload, pointer transforms, input routing, typed Android and
  Browser attachment refusal, texture lifecycle, and cleanup.
- Farm: `.50` (`172.20.0.50`), slot `arch010-vdi-display1`.
- Focused gate: `cargo test -p mde-shell-egui --features drm vdi::tests:: -- --nocapture`.
- Result: **PASS**, 33 passed, 0 failed.
- Boundary: this is fixture-backed VDI/DRM evidence; live guest scanout,
  physical input, and current-source seat deployment remain open.
