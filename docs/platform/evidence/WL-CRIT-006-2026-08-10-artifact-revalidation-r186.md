# WL-CRIT-006 — release artifact revalidation (r186)

Date: 2026-08-10

`install-helpers/release-evidence.sh validate` now re-opens every declared
release artifact and verifies that it is a regular, non-symlink file whose
size and SHA-256 still match the evidence descriptor. A refreshed outer
binding cannot make missing or replaced bytes authoritative when the CI gate
descriptor is absent.

## Verification

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=crit006-release-artifact-revalidation-r186
install-helpers/xcp-build.sh sync — passed
remote bash install-helpers/release-evidence.sh --self-test — passed
release-evidence: self-test passed (deterministic binding round-trip + fail-closed validation)
```

The self-test includes hostile missing-artifact and changed-artifact fixtures,
both with a refreshed evidence binding and without CI status provenance; both
were rejected. Local `bash -n` and `git diff --check` also passed.

## Limits

This proves the bounded validator and fixtures only. It does not prove a
signed production bundle, GitHub required-check publication, physical
three-seat acceptance, six-node lighthouse convergence, or corrected-forward
deployment.
