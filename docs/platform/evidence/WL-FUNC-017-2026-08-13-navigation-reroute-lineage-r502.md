# WL-FUNC-017 navigation reroute lineage — r502

Date: 2026-08-13

## Production gap closed

`NavigationConsumer::request_route` previously retracted an active route from
the Maps projection but always serialized the replacement as
`RouteRequestKind::Route` with no `replaces_route_id`. That bypassed the typed
reroute lineage required by the daemon and made an operator replacement
indistinguishable from a new route request.

The consumer now captures the active daemon-projected route identity before
retracting its geometry. It emits `Reroute` with that exact route ID; requests
made without an active route remain `Route` with no replacement lineage. The
existing replacement-flow regression now decodes the wire action and proves
the kind and route identity as well as generation and destination.

## Acceptance audit

- Origin and destination remain typed `RouteEndpoint` values and are validated
  by `RouteRequest::validate_at` before publication.
- Replacement/reroute now carries the exact active route identity and expected
  daemon generation. The old geometry is retracted immediately.
- Cancellation already targets the latest accepted daemon generation and is
  retained byte-for-byte until Bus publication succeeds.
- Maps remains a consumer only: it queues typed actions and folds validated,
  host-scoped, monotonic `NavigationSnapshot` projections. Provider work and
  route authority remain in `mackesd`.
- Wrong-host, stale, conflicting, and malformed snapshots fail closed without
  replacing the last accepted state. An `InterruptedByRestart` daemon snapshot
  projects an explicit unavailable state; retained actions cannot rebind to a
  foreign host after restart.

## Farm verification

- BigBoy `.130`, slot `func017-nav-route`:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func017-nav-route install-helpers/xcp-build.sh cargo test -p mde-maps-location-egui`
  — passed, 319 tests, 0 failed.
- `.50`, slot `func017-nav-clippy`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func017-nav-clippy install-helpers/xcp-build.sh cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings`
  — passed.

The first invocation of each gate used the Rust crate identifier
`mde_maps_location_egui`; Cargo rejected both before compilation and suggested
the canonical hyphenated package name used by the successful reruns above.

## Remaining acceptance

WL-FUNC-017 still requires the integrated first-release evidence bundle and
post-release live online/offline/reconnect navigation traces, Maps/Car proof,
MG90/provider-loss and restart/sleep/rejoin proof, package identity, and the
deferred direct-DRM review. Those release proofs are not inferred from this
focused farm gate.
