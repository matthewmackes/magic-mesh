# WL-FUNC-017 clock-adjacent weather launcher — 2026-08-08

The shell now folds the typed current-weather/effective-location projections
into one live weather icon and temperature target. It opens the existing Maps &
Location surface directly in Map → Weather mode; it creates no second launcher,
surface, or flyout. Stale, mismatched, absent, and unavailable projections never
fabricate a temperature or condition.

The operator-locked order in both taskbar placements is weather, live battery,
then time. Battery remains directly left of time; without a battery, weather
becomes clock-adjacent. Narrow layouts collapse weather to its icon while all
targets remain disjoint.

## Verification

- `.90`: focused projection, freshness, deep-link, responsive geometry, and
  top/bottom ordering proof passed 5/5.
- `.170`, slot `func017-weather-icons-s9-r1`: the closed weather icon registry
  passed 12/12, including unique identity/source and 16/24px tray raster proof.
- `.170`, slot `func017-weather-status-s9-r2`: the complete focused status-bar
  suite passed 25/25, including live battery and weather/battery/time geometry.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Installed live-seat captures with real weather projections at multiple widths
remain. FUNC-017 and UX-012 stay `Remaining`.
