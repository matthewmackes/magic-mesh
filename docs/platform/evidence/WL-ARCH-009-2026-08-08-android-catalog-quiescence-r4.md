# WL-ARCH-009 Android catalog quiescence — 2026-08-08

The optional Android catalog importer now blocks solely on shutdown when its
local trust configuration is absent. It remains registered and fail-closed,
but no longer wakes every second to rediscover that it cannot admit imports.

The configured path is unchanged: it replays a valid last-good catalog, polls
the bounded import lane, admits only signed newer revisions, and publishes the
typed state projection. The unconfigured path opens no Bus state and exits
promptly when its group receives shutdown.

## Focused farm verification

- Machine 193 (`172.20.0.90`), slot `arch009-android-fmt-r2`: target-file
  `rustfmt --check` passed. The repository-wide formatting check still reports
  unrelated pre-existing formatting drift outside this file, so it was not
  represented as a passing gate.
- Machine 9 (`172.20.0.50`), slot `arch009-android-quiesce-r1`:
  `workers::android_catalog::tests::unconfigured_worker_quiesces_without_creating_bus_state`
  passed 1/1 with 4,493 tests filtered. The test proves no Bus directory is
  created, the worker remains quiescent until cancellation, and cancellation
  joins successfully within one second.
- `git diff --check` passed. No tests were removed.

## Remaining acceptance gap

This closes one concrete optional-worker wake loop. The other environment- and
runtime-gated workers still need an implementation-by-implementation audit and
live idle/resource evidence before optional-worker quiescence is complete;
ARCH-009 stays `Remaining`.
