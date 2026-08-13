# WL-UX-011 — device-control generation re-attestation (r507)

Date: 2026-08-13

## Implemented result

The node-side device-control executor now re-reads provider inventory after
exact-body capability authorization and immediately before entering a hardware
effect seam. The final gate requires the same nonzero inventory generation,
category, device name, sysfs path, driver identity, and actionable provider
state that the operator previewed. If provider publication advances while
durable authorization is in flight, the consumed request fails closed and the
operator must preview and authorize the new generation.

This closes the preview-to-execution race without weakening cancellation,
authorization, audit, or result publication behavior.

## Farm evidence

- `.90`, slot `ux011-control-file-fmt2`:
  `CARGO_BUILD_JOBS=1 cargo test -p mackesd superseded_inventory_generation_is_re_attested_before_hardware -- --nocapture`
  passed 1/1 with 4,954 filtered.
- `.170`, slot `ux011-control-clippy`:
  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`
  passed.
- `.90`, slot `ux011-control-file-fmt2`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/device_control.rs`
  passed.
- `git diff --check` passed.

The first BigBoy attempt was abandoned when `.130` became unreachable under
farm saturation. An interrupted `.170` test was terminated and verified absent
before the exact test was routed to `.90`; no duplicate test remained active.

## Remaining WL-UX-011 acceptance

Complete the remaining real provider/control matrix and first-release package,
then perform the deferred non-blocking post-release one-node proof of live
preview, authorization, execution, cancellation, recovery, inventory refresh,
and safe failure behavior on installed hardware.
