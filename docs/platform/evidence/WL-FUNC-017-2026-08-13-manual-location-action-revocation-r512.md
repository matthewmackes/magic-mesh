# WL-FUNC-017 manual-location action generation revocation — r512

Date: 2026-08-13

## Production result

Maps now revokes a queued manual weather-location action when the effective
location projection no longer has the exact generation against which the
operator selected the offline result. A failed Bus publication can therefore
retry only while its compare-and-set authority is still current; a newer,
missing, or foreign location projection cannot leave an obsolete action
replaying on every refresh.

The change is confined to
`crates/desktop/mde-maps-location-egui/src/weather_ui.rs`. It adds no provider,
network, persistence, or render-path I/O.

## Farm evidence

- `.90`, slot `func017-weather-action`:
  `cargo test -p mde-maps-location-egui location_generation_change_revokes_pending_manual_action -- --nocapture`
  passed 1/1 with 319 filtered library tests and 0 failures.
- `.170`, slot `func017-weather-clippy`:
  `cargo clippy -p mde-maps-location-egui --bin mde-maps-location-egui -- -D warnings`
  passed.
- `.50`, slot `func017-weather-fmt`:
  `cargo fmt -p mde-maps-location-egui -- --check` passed.

The original BigBoy `.130` test route was stopped during rsync after it made no
progress while that host completed unrelated filesystem cleanup. The exact
test was rerouted to `.90`; no duplicate test was run.

## Remaining epic acceptance

WL-FUNC-017 still requires first-release package integration and the deferred,
non-blocking post-release one-node live matrix for manual/automatic/offline
location, provider loss/reconnect, navigation, Maps/Car, MG90, restart,
sleep/rejoin, package identity, and direct-DRM review.
