# WL-FUNC-017 current and forecast provider — 2026-08-08

A default-on daemon worker now publishes effective-location-bound current
conditions every five minutes and 120-hour/five-day forecasts every ten. A
location generation change makes both immediately due. Blocking NWS I/O and
JSON work stays off the async runtime; publication rechecks the exact host,
generation, coordinates, coverage, and timezone.

The provider enforces official HTTPS URLs, disabled redirects, timeout and body
bounds, provider-derived freshness, local-offset day grouping, absent rather
than zero-filled measurements, and bounded retry. An atomic non-symlink cache
recovers only an exact location generation; data is fresh through 90 minutes,
typed stale through six hours, then unavailable.

## Verification

BigBoy `.130`, slot `func017-weather-forecast-s3-r1`:

- `cargo test --locked -p mackesd --lib weather_forecast -- --nocapture`
  passed 8/8 twice (4,391 filtered).
- The suite covered off-runtime execution, independent cadence, generation
  refresh/race discard, restart cache age/isolation, hostile URLs/body bounds,
  timezone mismatch, DST-offset aggregation, and projection caps.
- `.196`, slot `func017-weather-contracts-r1`: the shared weather wire/admission
  contract passed 12/12 on the newly provisioned fifth farm node.
- `.90`, slot `func017-weather-workers-integration-r2`: the focused daemon
  weather integration gate passed 20/20 (4,403 unrelated tests filtered),
  covering location, forecast, and atmospheric workers together.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

No live NWS network/runtime fixture was exercised. Route-provider data, MG90,
Car integration, packaging, and release-seat proof remain, so FUNC-017 stays
`Remaining`.
