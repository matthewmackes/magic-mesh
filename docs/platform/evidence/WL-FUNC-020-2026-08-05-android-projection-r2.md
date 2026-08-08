# WL-FUNC-020 — Android Workloads projection r2 (2026-08-05)

The Workloads Android projection now carries the admitted workload's stable
target host through the catalog selection path. Projection remains deterministic:
blank identities are dropped, rows are matched by workload ID, duplicates are
sorted and collapsed, and launch remains disabled for stale, unavailable, or
unscoped inventory.

## Verification

- Farm `.50`, slot `wl-android-projection-r2`:
  `cargo test -p mde-shell-egui iac::android_apps::tests -- --nocapture`.
- Result: `11 passed; 0 failed; 0 ignored; 0 measured; 1438 filtered out`.
- The disposable farm workspace was removed after the run.
- This proves the typed projection and selection gate only; it does not claim a
  live Cuttlefish guest, ADB inventory, VDI session, or app launch on a seat.
