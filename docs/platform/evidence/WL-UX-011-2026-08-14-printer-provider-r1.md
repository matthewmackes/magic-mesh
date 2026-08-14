# WL-UX-011 Workers printer provider

- Farm `.90` slot: `ux011-printer-provider`.
- Gate: `cargo test -p mackesd --features async-services workers::device_inventory::tests::printer_provider -- --nocapture` — PASS, 2/2.
- The provider caps `lpstat` output/queues, validates names, publishes only
  coarse state, redacts URI/job/user material, and emits an explicit
  unavailable row on failure or oversize input.
