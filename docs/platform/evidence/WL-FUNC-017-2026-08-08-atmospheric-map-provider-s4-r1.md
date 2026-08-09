# WL-FUNC-017 atmospheric map provider — 2026-08-08

A default-on daemon worker now publishes bounded temperature, wind, and cloud
PNG snapshots for a latest-wins Maps viewport or deterministic location-derived
fallback, bound to both location and viewport generations. It uses the exact official nowCOAST NDFD WMS
products `ndfd_temperature:air_temperature`, `ndfd_wind:wind_velocity`, and
`ndfd_sky:total_sky_cover`; contract admission rejects path or layer drift.

The worker enforces `nowcoast.noaa.gov` HTTPS, no redirects, canonical WMS
1.3.0 parameters, response and PNG dimension/byte bounds, post-fetch authority
recheck, ten-minute cadence, bounded backoff, and an atomic identity-bound
fresh/stale/expired cache. Provider and cache work stays off the async runtime.

## Verification

- `.90`, slot `func017-atmosphere-contract-s4-r2`: 2/2 contract tests passed.
- BigBoy, slot `func017-weather-atmosphere-s4-r2`: 7/7 worker tests passed,
  including viewport churn and cache-identity proof; binary registration passed.
- The focused same-location/different-viewport cache rerun passed 1/1.
- Scoped rustfmt on `.170` and `git diff --check` passed.
- Live official GetCapabilities documents returned HTTP 200 and exposed the
  exact three product/layer identities above on 2026-08-08.

## Remaining acceptance gap

The typed action boundary and worker are ready for an interactive Maps producer,
but that GUI publication and selected PNG painting are not yet wired. WMS
responses expose no provider-valid timestamp, so freshness truthfully uses the
named render/fetch time. Live-seat proof remains; FUNC-017 stays `Remaining`.
