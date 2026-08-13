# WL-UX-011 typed power-supply provider — r489

Date: 2026-08-13

The reachable `hardware_probe` worker no longer assumes firmware-specific
`BAT0`/`BAT1` and `AC`/`ACAD` names. It discovers Linux power supplies by their
kernel `type`, sorts and caps the census at 64 entries, selects a stable valid
battery percentage, and reports external power only from an online non-battery
supply. This covers Surface and USB-C naming without credentials or control
authority and rejects impossible percentages above 100.

Farm evidence:

- `.170`, slot `ux011-power-provider-test-r489`: `cargo test -p mackesd
  --features async-services --lib
  workers::hardware_probe::tests::power_probe_is_typed_deterministic_and_bounded
  -- --exact --nocapture` passed 1/1 (4,928 filtered). An initial fixture run
  correctly failed because synthetic noise sources were marked online; the
  corrected hostile fixture keeps bounded noise offline and proves an online
  source beyond the lexical 64-entry budget is ignored.
- `.90`, slot `ux011-power-provider-clippy-r489`: `cargo clippy -p mackesd
  --features async-services --lib -- -D warnings` passed.
- `.130`, slot `ux011-power-provider-filefmt-r489`: `rustfmt --edition 2021
  --check crates/mesh/mackesd/src/workers/hardware_probe.rs` passed against the
  final file.

This is provider implementation evidence, not fleet acceptance. Remaining
WL-UX-011 acceptance includes the complete provider/control coverage matrix and
post-release physical fleet proof for safe generation-bound controls.
