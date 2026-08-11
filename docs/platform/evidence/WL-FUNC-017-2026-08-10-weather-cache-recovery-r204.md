# WL-FUNC-017 — malformed weather-cache recovery (r204)

- Scope: malformed regular weather caches are quarantined instead of being
  projected as stale truth; a fresh provider snapshot may then repair the cache.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func017-weather-cache-recovery-r204 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::weather_forecast::tests::malformed_regular_cache_is_quarantined_and_provider_refresh_recovers -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4732 filtered out`.
- Symlink and unsafe cache paths remain fail-closed.
