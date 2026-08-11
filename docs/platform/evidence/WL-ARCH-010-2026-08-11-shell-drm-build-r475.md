# WL-ARCH-010 DRM shell build evidence — 2026-08-11

- Scope: the shipped `mde-shell-egui` binary builds with the native DRM/KMS
  feature enabled, including the Display1 and EGL/GBM dependency path.
- Farm: `.50` (`172.20.0.50`), routed through `install-helpers/xcp-build.sh`.
- Focused gate: `install-helpers/xcp-build.sh cargo build -p mde-shell-egui --features drm`.
- Result: **PASS**; the `mde-shell-egui` dev binary finished successfully.
  Existing repository warnings remain, but there were no build errors.
- Boundary: this proves source-to-binary native shell wiring. It does not prove
  live KMS modeset, physical Display1 scanout, or current-source deployment to
  a seat.
