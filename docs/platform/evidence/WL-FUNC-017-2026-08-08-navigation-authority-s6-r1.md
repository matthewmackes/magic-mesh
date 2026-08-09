# WL-FUNC-017 daemon navigation authority — 2026-08-08

A reachable Workstation worker now owns bounded versioned route requests,
results, progress, reroute, cancellation, attribution, and latest-wins state on
the canonical node-scoped topics:

- `action/navigation/route/<host>`
- `action/navigation/progress/<host>`
- `action/navigation/cancel/<host>`
- `state/navigation/<host>`

The authority binds exact generation and request identity, rejects replay and
stale progress, persists state for restart recovery, and executes only through
an injected route provider. The production provider currently publishes an
explicit unavailable result; it does not fabricate a route or move provider I/O
into Maps rendering.

Maps now publishes canonical route/cancel intents off-render and folds bounded
state into immutable route geometry, maneuver, progress, cancellation, and
provider-unavailable presentation. Failed Bus writes retain the exact request
body for replay-safe retry; stale, wrong-host, and conflicting generations do
not replace the accepted route.

## Verification

- `.90`, slot `func017-navigation-s6-r1`: contract hostile/bounds tests passed
  2/2; route/progress/cancel/replay/restart/provider tests passed 3/3; reachable
  worker spawn/census proof passed 1/1.
- BigBoy `.130`, slot `func017-maps-navigation-consumer-s6-r1`: focused Maps
  route/cancel, generation/replay, projection, and refusal tests passed 3/3.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

An approved offline/online routing engine, verified route dataset, and live
route-matching progress source must be provisioned, followed by live
online/offline/reconnect traces. FUNC-017 stays `Remaining`.
