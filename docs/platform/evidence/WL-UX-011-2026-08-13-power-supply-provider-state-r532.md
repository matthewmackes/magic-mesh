# WL-UX-011 — bounded power-supply provider state (r532)

Date: 2026-08-13

## Executable acceptance slice

The production Device Inventory power-supply provider no longer treats every
`/sys/class/power_supply/*` entry as healthy or copies arbitrary sysfs text into
fleet-visible events. It now admits only bounded kernel-ABI supply types and
statuses, capacities in `0..=100`, boolean online state, and bounded printable
manufacturer/model identity. A battery or UPS without a valid capacity and
known charge status, an unknown supply type, or a non-battery supply without a
valid online bit remains visible with `DeviceStatus::Unknown` and the explicit
reason `power supply state unavailable`.

This closes a real S2 observation-provider gap: sourced physical power hardware
is still published, but missing or malformed provider facts cannot become
fabricated healthy state or credential-shaped event output.

## Scope

- `crates/mesh/mackesd/src/workers/device_inventory.rs`
- this evidence record

No shell, shared contract, worklist, release, Car, status-bar, Cargo, or
Collaboration path was changed.

## Farm evidence

All commands used explicit farm hosts and isolated slots.

- PASS — `.130` BigBoy, slot 3:
  `cargo test -p mackesd --features async-services power_supply_provider_reports_unavailable_and_filters_hostile_state -- --nocapture`
  — 1 passed, 0 failed, 4,968 filtered out. The fixture proves a valid battery
  remains healthy while an over-range capacity, unrecognized status containing
  credential-shaped text, and oversized manufacturer are suppressed and the
  row becomes explicitly unavailable.
- PASS — `.90`, slot 2:
  `cargo check -p mackesd --features async-services`.
- PASS — `.170`, slot 1:
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings`.
- Scoped diff hygiene: `git diff --check` passed.

The package-wide `cargo fmt -p mackesd -- --check` also inspected the tree but
reported pre-existing/concurrent formatting differences across many files
outside this slice. Those paths were not rewritten. The new block was aligned
manually to rustfmt's reported form; no unrelated formatting is claimed.

## Remaining WL-UX-011 acceptance

This slice does not close the epic. The provider coverage matrix still needs
direct production evidence or explicit unavailable blockers for every supported
Wi-Fi, audio, display, input, storage, printer, service, privacy, power, and
virtualization provider. Every exposed mutation must still have complete
preview/result, generation/capability, audit, cancellation, and recovery
evidence. Workers fleet rendering must still demonstrate stale/failed providers,
conflicts, history, scans, and credential-free exports; live one-node hardware
captures remain deferred until after the first release under the operator lock.
