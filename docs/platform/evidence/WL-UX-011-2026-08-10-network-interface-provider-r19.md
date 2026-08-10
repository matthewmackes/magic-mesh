# WL-UX-011 — physical network-interface provider (r19)

Date: 2026-08-10

Base revision: `45859dd3`

## Defect and correction

Device Inventory published PCI network controllers but did not publish the
physical `/sys/class/net` interfaces used by the generation-bound network
control path. The provider now adds at most 256 hardware-backed interfaces with
their exact sysfs identity, bound driver, wired/wireless kind, carrier, and
truthful kernel link state. Loopback and virtual links are excluded. The
provider never reads or publishes MAC addresses, SSIDs, NetworkManager
profiles, or credentials.

## Focused farm proof

Build VM `.90` (`172.20.0.90`), slot
`ux011-net-provider-r1-20260810`, passed:

```text
cargo test -p mackesd --lib workers::device_inventory::tests::physical_network_interfaces_are_bounded_sourced_and_credential_free -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 4671 filtered out
```

`rustfmt` and `git diff --check` passed for the owned source.

Source SHA-256:

- `d3ddb69fbc4cc6c179f7509418eaaaab937dc7ddadd465c840d5e770a7d1480b`
  — `crates/mesh/mackesd/src/workers/device_inventory.rs`

This is source/provider proof, not live-seat acceptance. Installed-seat
publication, Workers rendering, control compatibility, and the capped live
fleet proof remain open.
