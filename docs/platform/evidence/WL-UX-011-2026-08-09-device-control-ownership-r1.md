# WL-UX-011 checkpoint — device-control ownership binding (2026-08-09)

## Outcome

`device_control` now accepts a privileged target only when its host, category,
name, sysfs path, and driver exactly match the executing provider's published
inventory. A requester can no longer borrow a real sysfs path while substituting
another provider-owned identity or arbitrary driver, and a foreign-host request
fails before planning or mutation.

Hostile temp-sysfs tests prove forged properties and foreign ownership leave the
device's `authorized` control file unchanged. The valid canonical target still
executes through the same real write seam.

## Source

- `crates/mesh/mackesd/src/workers/device_control.rs`
- SHA-256: `78f1cde14057f7e473b1dfc4f671c2ce13e74c80f325ef1b313247c4d281cc36`

## Farm verification

- Host: `172.20.0.90`
- Slot: `ux011-device-control-ownership-r1-20260809`
- `cargo test -p mackesd --lib workers::device_control::tests -- --nocapture`
  — **16 passed, 0 failed**.
- `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/device_control.rs`
  — **passed**.

The broader `cargo fmt -p mackesd -- --check` remains blocked by pre-existing
formatting drift in untouched modules. A non-`--lib` test invocation also hits
the existing cloud-gate export failure while compiling the binary target; the
focused library tests above compile and exercise the production worker module.
