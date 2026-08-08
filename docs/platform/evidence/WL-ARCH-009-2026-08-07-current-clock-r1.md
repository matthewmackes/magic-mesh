# WL-ARCH-009 current Eastern clock verification (2026-08-07)

The Construct clock correction is in the fresh Fedora 44 release-5 artifact
deployed to Dell and seat 15. `ClockZone::EasternStandard` now applies the
daylight-aware US Eastern offset (`UTC-04:00` in August and `UTC-05:00` in
winter) before the shared `HH:MM` fold; retained chat timestamps use the offset
for their own timestamp rather than the current offset.

## Farm verification

- Host: `172.20.0.90`
- Slot: `construct-eastern-clock-current-r1`
- Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=construct-eastern-clock-current-r1 ./install-helpers/xcp-build.sh cargo test -p mde-shell-egui us_time_zones_follow_daylight_saving_rules --locked -- --nocapture`
- Result: `1 passed; 0 failed; 1457 filtered out`

The test asserts August 2026 Eastern is `-04:00`, January 2026 is `-05:00`,
and UTC remains zero. The source is in
`crates/desktop/mde-shell-egui/src/timers.rs`; the visible taskbar and Timers
surface both consume `display_unix()` and `hhmm()` from that one clock path.

## Installed artifact binding

- RPM: `magic-mesh-12.1.6-5.x86_64.rpm`
- SHA-256: `8219d399ae7abf498f4916c9c43240628bbef02e9ef71971d235db3ada450be3`
- Dell `172.20.146.225`: installed, `rpm -V magic-mesh` clean,
  `mde-shell-egui.service` active
- Seat 15 `172.20.0.15`: installed, `rpm -V magic-mesh` clean,
  `mde-shell-egui.service` active

At the live check, both hosts reported `2026-08-07 11:30 EDT -0400`; neither
had a persisted clock override, so the deployed default was Eastern Time.

This is farm/source and installed-binary proof; a direct screenshot of the
Construct clock was not available in this deployment pass.
