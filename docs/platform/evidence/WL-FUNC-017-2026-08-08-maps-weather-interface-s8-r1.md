# WL-FUNC-017 Maps weather interface — 2026-08-08

Maps now owns a map-first Weather mode with Current, 1D, 3D, and 5D views. It
folds the bounded effective-location, current-condition, forecast, and
atmospheric projections outside render work; reports fresh, stale, or
unavailable state honestly; and retains provider attribution.

Temperature, wind, and cloud are one exclusive atmospheric selector, while
radar and alerts remain independent toggles. The responsive forecast sheet and
location summary do not create a second weather surface.

The interactive map now publishes a latest-wins typed viewport and paints only
the selected atmospheric PNG after off-render decode. Both location and
viewport generations bind admission, cache reuse, and race refusal.

Manual location search is explicit-submit and offline. It caps queries at 4
KiB and results at 24, accepts only gazetteer entries with a verified place ID,
IANA time zone, coordinates, and NWS coverage, and publishes the exact typed
`action/weather/set-location` request. Missing metadata and empty results stay
truthfully unavailable; no network geocoder or parallel location authority was
introduced.

## Verification

- BigBoy focused Maps weather model/view proof passed 4/4.
- Focused viewport publication passed 1/1; PNG decode/paint/race proof passed
  6/6 in slot `func017-maps-weather-png-r1`.
- `.90`, slot `func017-weather-manual-search-r1`: manual-search focused tests
  passed 5/5 and complete filtered runs passed 7/7.
- `.90`, slot `func017-maps-integration-r1`: the complete Maps package passed
  295/295 after the manual-search, weather-map, and offline-cache integration.
- Scoped formatting and `git diff --check` passed.

## Remaining acceptance gap

Responsive captures and installed end-to-end Maps -> Bus -> mackesd -> Weather,
nowCOAST, and seat proof remain. FUNC-017 stays `Remaining`.
