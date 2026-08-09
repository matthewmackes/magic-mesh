# WL-UX-012 live battery placement — 2026-08-08

The shell now folds the existing off-render UPower snapshot into one primary
power-supply battery indicator. A valid live reading renders its charge icon
and percentage immediately left of the clock in both Bottom and Left taskbar
placements; an absent or non-finite reading renders no fabricated status.
The weather launcher preserves the operator-locked order as weather, battery,
time; when weather is present, battery remains directly adjacent to time.

## Verification

- `.90`, slot `live-battery-left-clock-r1`: focused status-bar proof passed
  24/24, including primary-battery selection, charging icon selection, honest
  absence, and exact battery/clock edge ordering in both placements.
- Scoped `git diff --check` passed.
- `.170`, slot `func017-weather-status-s9-r2`: the complete status-bar suite
  passed 25/25, including weather/battery/clock ordering and narrow fallback.
- `.90`, slot `bug-battery-left-of-time-r2`: the operator-reported placement
  regression was rechecked against the current integrated tree with the single
  exact shell-binary geometry test; 1/1 passed and 1,474 unrelated tests were
  filtered.
- `.90`, slot `integrated-battery-clock-s6-r1`: the current integrated tree
  repeated the fully-qualified exact geometry test; 1/1 passed with 1,492
  unrelated tests filtered.

## Remaining acceptance gap

Live-seat visual capture and the broader taskbar acceptance matrix remain, so
UX-012 stays `Remaining`.
