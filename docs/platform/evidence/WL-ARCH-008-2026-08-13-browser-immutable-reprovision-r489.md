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
- `172.20.0.196`, slot `arch008-browser-module-r489`: the first broader Browser
  module run exposed a public error-contract mismatch in the new strict-read
  path (10 passed, 1 failed). The message was corrected to preserve the existing
  `could not persist` contract; the exact failed test then passed 1/1 in the
  warm slot with 4,928 filtered out.

## Changed files

- `crates/mesh/mackesd/src/workers/cloud/verbs/browser.rs`
- `docs/platform/evidence/WL-ARCH-008-2026-08-13-browser-immutable-reprovision-r489.md`

## Remaining acceptance

This closes the Browser VM reprovision identity boundary only. WL-ARCH-008 still
requires portable migration/import quality, reproducible guest image and audio
readiness, shell VDI behavior, standalone-stack provenance, negative production
reachability/package proof, and the deferred post-release live performance and
upgrade measurements described by S1-S6.
