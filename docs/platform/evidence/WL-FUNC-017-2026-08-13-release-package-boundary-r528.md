# WL-FUNC-017 — governed offline Maps release package boundary (r528)

Date: 2026-08-13

The first-release input preflight now requires an immutable Maps approval,
external tile source root, positive quota, and production verifier. Before any
release build mutation it regenerates the governed bundle with
`produce-offline-catalog.py`, then admits and no-replace materializes that exact
bundle for the pinned source revision and epoch with
`materialize-offline-catalog.py`. Arbitrary tile bytes remain outside Git and
RPM manifests.

The Workstation and Server RPM payloads now own the producer, materializer, and
compiled production verifier at fixed `/usr/libexec/mackesd` paths. The existing
RPM payload gate checks both manifest variants, rejects embedded tile assets,
and inspects real RPM file lists when supplied. The package release is 35.

Hostile coverage proves that a missing approval, approval for another source
revision, substituted tile bytes, verifier refusal, or incomplete Maps package
payload cannot reach the release build boundary.

## Exact farm gates

- `.50`, slot `func017-maps-preflight-r528c`:
  `install-helpers/test-release-input-preflight.sh` — PASS.
- `.170`, slot `func017-maps-rpm-payload-r528d`:
  `install-helpers/verify-rpm-payload.sh maps-package && install-helpers/verify-rpm-payload.sh --self-test` — PASS.
- `.50`, slot `func017-maps-shellcheck-r528e`:
  `shellcheck install-helpers/release-input-preflight.sh install-helpers/test-release-input-preflight.sh install-helpers/verify-rpm-payload.sh` — PASS.
- Local tiny checks: Bash syntax and `git diff --check` — PASS.

Post-release live Maps/navigation/weather/MG90 proof remains deferred and
non-blocking under the operator's first-release directive.
