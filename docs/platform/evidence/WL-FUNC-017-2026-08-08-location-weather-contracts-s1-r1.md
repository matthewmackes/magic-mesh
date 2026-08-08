# WL-FUNC-017 location and weather contracts — 2026-08-08

The shared mesh contract crate now defines bounded version-1 contracts for
weather location preference, effective location, current conditions, and the
general 120-hour/five-day forecast. Stable helpers cover
`action/weather/set-location` and latest-wins
`state/weather/{location,current,forecast,map}/<host>`. The existing Car
drive-ahead `state/overlay/nws-hourly/<host>` contract is unchanged.

Auto mode admits only a fresh same-host GNSS observation or a saved verified
place; Manual mode requires a verified place. Projections carry location
generation, producer/fetch/source times, attribution, explicit gaps, typed
fresh/stale/unavailable state, local date/time-zone identity, and unit-tagged
optional measurements. Missing measurements are absent rather than zero-filled.

Unknown fields and versions, recursive duplicate JSON keys, hostile or
non-finite coordinates, wrong-host/stale/future fixes, impossible units,
oversized wires/collections, duplicate periods, and inconsistent availability
relationships fail closed.

## Focused farm verification

BigBoy `.130`, slot `func017-location-weather-contracts-r1`:

- Full `mackes-mesh-types` crate: 454 passed, 0 failed.
- Location contract filter: 7 passed, 0 failed.
- Weather contract filter: 8 passed, 0 failed.
- Rustfmt and scoped `git diff --check`: passed.

## Source hashes

```text
7e5ffd007a6615b922f1b21b2ccd06303fa65f7b0537a0c68f0169184e7ffd26  crates/mesh/mackes-mesh-types/src/location.rs
23d96be47a70942baca8ee67817ea3663d7c78c5f96cac606dcb13ebfbee331f  crates/mesh/mackes-mesh-types/src/weather.rs
0817138c0494c5cb93d5ecd1af98bee5c2b863d54e0ed6abc689ba075f5993d6  crates/mesh/mackes-mesh-types/src/lib.rs
```

## Remaining acceptance gap

This advances FUNC-017 S1 and freezes the location/weather projection surface;
it does not implement S2's daemon resolver or persistence. Provider workers,
NWS aggregation, weather map layers, offline maps/routes, MG90 recovery, Maps
and taskbar UI, packaging, and live evidence remain. FUNC-017 remains
`Remaining`.
