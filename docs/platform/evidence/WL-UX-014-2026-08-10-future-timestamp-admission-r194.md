# WL-UX-014 future-timestamp admission — 2026-08-10 r194

## Production correction

The KIRON ToastHost boundary now rejects a typed health lower third when its
`active_since_ms` or `observed_at_ms` is ahead of the seat clock used for
admission. UX-013 remains the authority for health state and timestamp
semantics; UX-014 only prevents a future-dated payload from entering the
cinematic renderer, where it could show a false alert or derive an invalid
dwell timeline.

## Farm proof

- Host: `172.20.0.50`
- Slot: `ux014-future-timestamp-admission-r194`
- Focused regression:
  `workers::toast_bridge::tests::typed_health_marker_rejects_future_dated_lower_thirds`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 1546 filtered out`
  (the regression covers both future lifecycle and observation cases)
- Scope: UX-014 ToastHost admission only; no UX-013 health recalculation,
  second queue, asset, renderer-tier, audio, package, or live-seat claim.
