# WL-ARCH-008 Browser immutable reprovision boundary — 2026-08-13

## Scope

The typed `browser-provision` handler now treats the admitted `browser-vm`
desired document as an immutable lifecycle identity. An exact request replay is
an idempotent success, but a later provision request cannot silently replace the
existing VM with a different image digest or profile. Replacement requires the
typed Workload lifecycle to remove the admitted declaration first. Existing
desired state is read through the strict reader, so malformed or unsafe state
fails closed without mutation.

## Farm gates

- BigBoy `172.20.0.130`, slot
  `arch008-browser-immutable-reprovision-r489`:
  `cargo test -p mackesd --locked workers::cloud::verbs::browser::tests::browser_reprovision_is_idempotent_but_cannot_retarget_the_admitted_vm --lib -- --exact --nocapture`
  passed 1/1 with 4,928 filtered out.
- `172.20.0.90`, slot `arch008-browser-immutable-clippy-r489`:
  `cargo clippy -p mackesd --locked --lib -- -D warnings` passed.
- `172.20.0.170`, slot `arch008-browser-immutable-fmt-r489`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cloud/verbs/browser.rs`
  passed after explicit farm sync.
- `172.20.0.196`, slot `arch008-browser-module-r489`: the broader Browser
  module test was launched as an additional unique gate and remained in cold
  compilation when the required focused gates completed; it is not used as
  evidence for this commit.

## Changed files

- `crates/mesh/mackesd/src/workers/cloud/verbs/browser.rs`
- `docs/platform/evidence/WL-ARCH-008-2026-08-13-browser-immutable-reprovision-r489.md`

## Remaining acceptance

This closes the Browser VM reprovision identity boundary only. WL-ARCH-008 still
requires portable migration/import quality, reproducible guest image and audio
readiness, shell VDI behavior, standalone-stack provenance, negative production
reachability/package proof, and the deferred post-release live performance and
upgrade measurements described by S1-S6.
