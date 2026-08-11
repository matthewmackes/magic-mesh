# WL-FUNC-017 evidence — future cache fallback (r219)

- Scope: weather restart/cache fallback.
- Change: future-dated nested current observations and forecast production
  timestamps are refused during provider outage; typed unavailable projections
  are published instead of hostile cache data being treated as fresh.
- Farm host: `172.20.0.50`.
- Farm slot: `func017-weather-future-cache-r219-final`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func017-weather-future-cache-r219-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::weather_forecast::tests::restart_refuses_future_dated_cache_snapshots_during_provider_outage -- --exact --nocapture`
- Result: `1 passed; 0 failed; 4747 filtered out`.
