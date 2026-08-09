# WL-ARCH-010 cloud production compile — r14

Date: 2026-08-09

## Scope

Reproduce and correct the production-only `mackesd` cloud compilation failure
exposed by the governed candidate build. Verification was pinned to BigBoy
`172.20.0.130`, slot `arch010-cloud-compile-r14`.

## Root cause

`crates/mesh/mackesd/src/workers/cloud/mod.rs` placed the crate-visible export
of the cloud authorization gate behind `#[cfg(test)]`. The underlying
`cloud/gate.rs` implementation is production code, and production consumers in
the cloud worker, IPC action authorization, host state, workload compute,
action dispatch, and first-desktop onboarding import those same primitives
through `workers::cloud`.

The initial locked production check failed with 25 E0432/E0433/E0405/E0425
errors. Missing names were `claim_nonce`, `placement_match`, `verify_token`,
`HmacTokenSigner`, `NullSigner`, `Placement`, `TokenSigner`, `TokenVerdict`, and
`DEFAULT_AUTH_ROOT`; rustc identified the `#[cfg(test)]` re-export as the
configured-out item.

## Correction

Removed only the erroneous `#[cfg(test)]` attribute from the existing
`pub(crate) use gate::{...}` statement. Visibility remains crate-local. No gate
logic, token verification, replay protection, credential loading, placement
matching, authorization fallback, feature selection, or dependency changed.

## Focused BigBoy gate

Command:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-cloud-compile-r14 install-helpers/xcp-build.sh cargo check -p mackesd --lib --features async-services --locked
```

- Before correction: failed with 25 production reachability errors.
- After correction: passed, exit 0, `Finished dev profile` in 41.73 seconds.
- Existing warnings remained non-fatal and were outside this cloud export scope.

No broad test, package build, candidate signing, deployment, or live seat
mutation was performed.

## Remaining blockers

This removes the cloud production compilation blocker only. It does not provide
the publisher credential/attestation, complete the governed candidate workflow,
sign a candidate, or prove deployment on live seats.
