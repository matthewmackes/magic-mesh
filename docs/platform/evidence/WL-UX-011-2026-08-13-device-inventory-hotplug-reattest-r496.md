# WL-UX-011 — Device inventory hotplug re-attestation (r496)

Date: 2026-08-13

## Implemented boundary

`device_inventory` now re-attests every claimed sysfs identity during final
inventory reconciliation. If a kernel object disappears after its provider
attributes were enumerated but before the generation is assembled, the stale
record is revoked instead of being published under its no-longer-live textual
path. Records that genuinely have no sysfs identity remain admissible, and an
unrelated still-present provider remains published.

The focused regression models two input providers, removes one after provider
enumeration, and proves that reconciliation publishes only the still-live
identity.

## Farm evidence

- `172.20.0.50`, slot `ux011-device-hotplug-test-r496b`:
  `cargo test -p mackesd --lib workers::device_inventory::tests::hot_unplugged_sysfs_identity_is_revoked_before_publication -- --exact --nocapture`
  passed 1/1 with 4,940 filtered out.
- `172.20.0.170`, slot `ux011-device-hotplug-clippy-r496`:
  `cargo clippy -p mackesd --lib --no-deps -- -D warnings` passed.
- `172.20.0.50`, slot `ux011-device-hotplug-fmt-r496b`: a file-scoped
  `rustfmt` comparison proved the changed hunks clean. Pre-existing formatting
  drift elsewhere in `device_inventory.rs` was not folded into this slice.

BigBoy (`172.20.0.130`) was excluded from final evidence after its toolchain
reported `bare`. The initial `.130` and `.196` sync attempts were also safely
refused below the farm's 8-GiB free-space floor and support no acceptance claim.

## Remaining epic acceptance

WL-UX-011 still requires the broader credential-free provider/control coverage
matrix and the deferred post-release physical-seat proof. This checkpoint
closes only mid-probe hot-unplug freshness for the reachable Device Inventory
producer.
