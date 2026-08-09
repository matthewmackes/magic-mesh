# WL-FUNC-017 weather cache identity recovery — 2026-08-09

Revision under test: `d0f5fdd5e72e8e9fe86802ceb0bb431a11cc752a`
plus the scoped `weather_forecast.rs` correction recorded by this evidence.

## Correction

Weather restart recovery previously admitted a cached current or forecast
snapshot after checking only the outer cache envelope. A hostile or partially
corrupted cache could keep a valid host/location envelope while embedding a
snapshot from another host, location generation, point, or timezone.

The worker now binds each embedded snapshot back to the effective-location
authority before it can be reused:

- current conditions must match host, generation, and point;
- forecasts must also match the authoritative timezone;
- a mismatch is treated as unavailable provider data, with no cached
  conditions, hourly periods, or daily summaries exposed.

This keeps restart recovery idempotent: the current authority receives one
typed unavailable projection instead of data retained from a different
authority generation.

## Focused farm verification

Farm host `172.20.0.170` (machine 194), slot `weather-func017-r18`:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=weather-func017-r18 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  workers::weather_forecast::tests::restart_refuses_hostile_nested_cache_identity_even_when_envelope_matches \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,395 filtered out` in the test binary. The
fixture writes a valid cache, tampers only the nested current generation and
forecast timezone, restarts at provider-stale age, and proves both projections
remain bound to generation 7 while exposing no hostile cached payload.

Scoped `git diff --check` passed. No broad or duplicate test suite was run.

## Remaining acceptance gap

This is a restart/cache authority boundary only. It does not claim live NWS,
MG90, package, release-seat, or five-seat acceptance, so WL-FUNC-017 remains
`Remaining`.
