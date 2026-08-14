# WL-UX-011 Workers service provider

- BigBoy `.130` farm slot: `ux011-services-provider-clean`.
- Gate: `cargo test -p mackesd --features async-services workers::device_inventory::tests::service_provider -- --nocapture` — PASS, 1/1.
- The provider uses a read-only `systemctl list-units` observation with a
  two-second command deadline and 128 KiB output cap; it allowlists unit names,
  publishes only coarse active/failed/unknown state, excludes descriptions, and
  emits an explicit unavailable row on failure or oversize input.
- The gate also ran the mackesd test harness with the targeted test selected:
  5,002 tests filtered, targeted provider test passed, zero failures.
