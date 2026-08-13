# WL-UX-011 — truthful Wi-Fi provider state (r539)

Date: 2026-08-13

## Result

The existing hardware-probe cadence now publishes a bounded, credential-free
`wifi-provider/<node>.json` projection. NetworkManager supplies radio and device
readiness while `/sys/class/net/*/wireless` supplies kernel device identity.
`Ready` requires those independent inventories to agree and a connected
NetworkManager device. Explicit radio-off and proven hardware absence publish
`Disabled`; enabled-without-link publishes `Disconnected`. Missing, malformed,
oversized, duplicate, substituted, or contradictory facts publish `Unknown`.
The provider reads no SSIDs, profiles, or secrets and has no mutation authority.

## Farm gates

- `.90` slot 1: `cargo test -p mackesd --features async-services hostile_or_contradictory_wifi_facts_fail_unknown -- --nocapture` — passed 1/1.
- BigBoy `.130` slot 1: `cargo clippy -p mackesd --features async-services --all-targets -- -D warnings` — passed.
- `.90` slot 1: `cargo build -p mackesd --features async-services --all-targets` — passed.
- `.170` slot 1: exact owned production files passed Rustfmt 1.94 `--check`; crate-wide formatting remains pre-existing red outside this slice.
- Scoped `git diff --check` — passed.

## Deferred acceptance

Physical Wi-Fi transitions and one-node installed-release observation remain
post-release, non-blocking acceptance. Remaining WL-UX-011 coding still includes
audio, display, printers, services, privacy, virtualization, and additional
capability-gated safe-control coverage.
