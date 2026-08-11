# WL-FUNC-017 — offline basemap cache revalidation (r215)

- Scope: cached MBTiles metadata revalidates regular-file identity and reloads
  after an atomic bundle replacement instead of retaining stale metadata.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func017-offline-basemap-cache-revalidation-r215 install-helpers/xcp-build.sh cargo test -p mde-maps-location-egui --lib basemap::tests::cached_metadata_reloads_after_atomic_mbtiles_replacement -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 311 filtered out`.
