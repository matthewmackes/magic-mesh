# WL-FUNC-017 atmospheric cache quarantine — 2026-08-11

- Scope: a malformed regular atmospheric-map cache is atomically moved away
  from the authority path before provider-outage fallback. The corrupt bytes
  remain available as quarantine evidence; symlink and non-regular paths remain
  fail-closed.
- Farm: `172.20.0.90`, slot `1`.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::weather_atmosphere::tests::malformed_regular_cache_is_quarantined_before_provider_outage_fallback -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,793 filtered out.
