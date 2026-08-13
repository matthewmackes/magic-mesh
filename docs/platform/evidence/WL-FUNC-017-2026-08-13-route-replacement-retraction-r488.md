# WL-FUNC-017 route replacement retraction — r488

Date: 2026-08-13

## Acceptance gap

`NavigationConsumer::request_route` previously retained the accepted route while
queuing a request for a different destination. Until the daemon published its
next `Calculating` or terminal snapshot, Maps could therefore paint the prior
route geometry and maneuver under a `Route request queued` status. This violated
S6's requirement that navigation present no stale route.

## Implementation

The Maps navigation consumer now retracts its projected route atomically when a
replacement route intent is admitted. The accepted generation remains bound to
the consumer, so the outgoing request still targets the daemon's latest
generation and retains the existing byte-identical Bus retry behavior. Cancel
semantics are unchanged.

The focused regression starts from an accepted active generation, queues a new
destination, proves the prior route is immediately absent, and verifies the
materialized action still carries the accepted generation and new destination.

## Farm evidence

- `.50`, slot `func017-route-retract-test-r488`:
  `cargo test -p mde-maps-location-egui navigation_ui::tests::replacement_request_retracts_prior_route_before_publication -- --exact --nocapture`
  passed 1/1 (315 filtered out).
- BigBoy `.130`, slot `func017-route-retract-clippy-r488`:
  `cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings`
  passed.
- `.196`, slot `func017-route-retract-fmt-r488`: direct farm-side
  `rustfmt --edition 2021 --check crates/desktop/mde-maps-location-egui/src/navigation_ui.rs`
  passed. Package-wide fmt also exposed pre-existing unrelated drift in
  `offline_cache.rs`; that file was not modified by this slice.

The initial focused command used an unqualified exact filter and selected zero
tests. It is not counted as evidence; the corrected module-qualified 1/1 run
above is the gate of record.

## Remaining epic acceptance

This slice closes stale-route presentation during destination replacement. The
epic still requires provisioned offline map/provider data, configured MG90
manager and hardware recovery proof, live NWS/atmospheric publication, direct
DRM Maps/weather/navigation review, and post-release seat evidence for the
clock deep link and Car radio/route health.
