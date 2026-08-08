# WL-FUNC-021 — NWS no-fix retry phase and bounded backoff (2026-08-07)

## Finding

The forecast worker’s no-fix path used the same immediate retry cadence on
every seat. A simultaneous outage could therefore align NWS requests and
degraded-state work across the fleet.

## Change

`nws_forecast_overlay` now derives a stable per-host phase below 20 seconds for
the first no-fix retry after startup or recovery, then uses bounded exponential
backoff from 20 seconds through the existing 10-minute ceiling. Successful
polls reset the retry budget. Immediate degraded publication and interruptible
shutdown remain unchanged.

## Verification

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=nws-forecast-recovery-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  nws_forecast_overlay --features async-services --locked -- --nocapture
```

Result: 12 passed, 0 failed, 4,404 filtered out. The focused tests cover phase
stability/bounds, retry ceiling, backoff progression, and short-poll caps.

This is farm/source evidence only. Live NWS recovery, post-restart CPU proof,
and Dell acceptance remain open while the authorized endpoints are unreachable.
