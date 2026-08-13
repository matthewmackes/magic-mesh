# WL-FUNC-017 — Car radio freshness projection (r494)

Date: 2026-08-13

## Executable gap

The typed MG90 v2 fold already downgraded expired radio inventory and removed
its active-path claim, but the reachable Car status catalog still rendered
operational values directly from the retained legacy mirror. After freshness
became stale or unknown, plausible carrier, address, signal, WAN, link-health,
Wi-Fi/VPN, latency/loss, failover, and usage values could therefore continue to
look current.

## Implementation

`crates/desktop/mde-maps-location-egui/src/car_status.rs` now requires a fresh
typed radio-domain projection for every operational MG90 radio tile. Stale or
unproven snapshots render unavailable with a muted tone. The explicit Radio
Health diagnostic remains visible so the operator retains the bounded stale,
unsupported, offline, or unavailable reason instead of receiving a silent
blank.

The behavior is runtime-reachable through `CarStatusItem::display`/`value`, the
same catalog consumed by the driver's selectable status strip.

## Farm gates

- `172.20.0.50`, slot `func017-car-radio-freshness-test-r494`:
  `cargo test -p mde-maps-location-egui stale_or_unproven_radio_snapshot_revokes_all_operational_tiles -- --nocapture`
  passed 1/1 (316 filtered out). The initial `--exact` invocation selected zero
  tests and was explicitly rejected as evidence before this corrected run.
- `172.20.0.130`, slot `func017-car-radio-freshness-clippy-r494`:
  `cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings` passed.
- `172.20.0.50`, slot `func017-car-radio-freshness-filefmt-r494`:
  `rustfmt --edition 2021 --check crates/desktop/mde-maps-location-egui/src/car_status.rs`
  passed.

Package-wide fmt also exposed pre-existing formatting drift in
`offline_cache.rs`; that unrelated file was not changed. All concurrent shared
worktree edits were preserved.

## Remaining epic acceptance

This closes one MG90 Car-projection freshness gap. `WL-FUNC-017` still requires
the remaining offline maps/navigation/weather work and the deferred post-release
installed/live MG90, weather-provider, restart, sleep/rejoin, and package-upgrade
acceptance matrix recorded by the canonical worklist.
