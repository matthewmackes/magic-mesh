# WL-ARCH-010 native DRM/PRIME boundary evidence — 2026-08-11

- Scope: the optional `mde-egui` native DRM feature compiles and its Display1
  shell-side PRIME/KMS boundary remains fail-closed.
- Focused gates on `.90` (`172.20.0.90`):
  `cargo test -p mde-egui --features drm drm::tests::external_dmabuf_metadata_is_bounded_before_prime_import -- --exact --nocapture`,
  `cargo test -p mde-egui --features drm drm::tests::prime_import_liveness_degrades_cleanly -- --exact --nocapture`,
  and `cargo test -p mde-egui --features drm drm::tests::display1_source_failure_cleans_native_scanout_before_returning -- --exact --nocapture`.
- Result: **PASS**, 3 passed, 0 failed; the native DRM feature compiled with
  GBM/EGL/DRM dependencies.
- Boundary: this proves bounded metadata, graceful PRIME degradation, and
  ordered cleanup in the native seam. It does not prove live KMS/EGL scanout,
  physical Display1 presentation, or seat acceptance.
