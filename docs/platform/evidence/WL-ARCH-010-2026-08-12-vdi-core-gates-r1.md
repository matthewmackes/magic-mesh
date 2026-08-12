# WL-ARCH-010 — mde-vdi-core focused farm gates (2026-08-12)

## Scope

This slice verifies the executable `mde-vdi-core` input and damage/pixel
contracts already wired into the ARCH-010 VDI boundary. It changes no source
code and does not claim live KMS/Display1 or seat acceptance proof.

## Farm evidence

Both gates were run on build VM `.90` with independent slots and the required
locked dependency graph:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-vdi-test-20260812b MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test -p mde-vdi-core --locked
Finished `test` profile ... in 3m 09s
27 passed, 0 failed; doc-tests 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-vdi-clippy-20260812c MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo clippy -p mde-vdi-core --locked --lib
Finished `dev` profile ... in 1m 28s
exit 0; warnings only
```

The warnings are pre-existing lint warnings in `mde-egui` and one
`mde-vdi-core` documentation warning; no clippy error or denied lint occurred.

## Acceptance status

The focused implementation gate is green. Live KMS/Display1 behavior and
post-release acceptance remain separate acceptance criteria and are deferred
until the first full release under the active release policy.
