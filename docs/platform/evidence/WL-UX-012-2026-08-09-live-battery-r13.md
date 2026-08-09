# WL-UX-012 live battery audit — r13

Date: 2026-08-09

## Outcome

The existing production implementation already satisfies the operator request,
so this audit made no production-code change.

- `SystemState::poll` runs independently of the active surface and drains the
  latest snapshot from `SnapshotPump`; UPower/seat reads therefore remain off
  the render thread.
- `Shell::mount_nav_bar_slot` and `Shell::mount_status_bar_slot` fold the latest
  typed battery snapshot through `LiveBatteryStatus::from_batteries` before
  painting the bottom and top placements respectively.
- The fold selects the first primary UPower power-supply battery, rejects a
  missing primary battery and non-finite charge, and does not fabricate an
  absent indicator.
- Both placement layouts reserve the battery cell immediately left of the clock.
  Weather is placed to its left, while clock, battery, weather, bell, and status
  controls use disjoint rectangles.

## Focused farm verification

Host: machine 193 (`172.20.0.90`)

Slot: `ux-battery-r13`

- `cargo test -p mde-shell-egui status_bar::tests::live_battery_uses_primary_upower_reading_and_charging_icon -- --exact --nocapture`
  — 1 passed, 0 failed.
- `cargo test -p mde-shell-egui status_bar::tests::weather_then_battery_then_time_is_disjoint_in_both_placements -- --exact --nocapture`
  — 1 passed, 0 failed.

The initial cold-build invocation used an incomplete exact test path and ran
zero tests; it is not acceptance evidence. The two fully qualified commands
above are the success-critical proof.
