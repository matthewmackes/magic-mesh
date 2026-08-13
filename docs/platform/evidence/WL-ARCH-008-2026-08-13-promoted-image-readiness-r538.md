# WL-ARCH-008 — promoted Browser image readiness (r538)

Date: 2026-08-13

## Production slice

`install-helpers/request-browser-vm-workload.sh` no longer treats a syntactically
valid `browser-vm-chromium:VERSION` as sufficient image readiness. Before
authoring a `start` or `start_and_attach` Workloads request, it now requires the
selected version to be the exact catalog `PROMOTED` generation and verifies the
bounded `catalog-admission.json`, identity-manifest SHA-256, artifact byte count,
identity-manifest artifact identity, and the complete promoted image SHA-256.
Missing, stale, symlinked, writable, malformed, or substituted catalog inputs
fail closed before Bus publication. Existing-generation lifecycle operations do
not accept a replacement image.

The focused hostile regression constructs one valid promoted catalog, admits
it, mutates the image bytes without updating its signed/release-derived catalog
identity, and proves that readiness rejects the substituted generation.

## Farm gates

- `.90`, workspace `magic-mesh-farm-arch008-static-r538`:
  `bash -n install-helpers/request-browser-vm-workload.sh` — passed.
- `.90`, same workspace:
  `install-helpers/request-browser-vm-workload.sh --self-test` — passed,
  including the exact promoted-image admission and hostile digest substitution.
- `.90`, same workspace:
  `install-helpers/verify-rpm-payload.sh browser-vm-payload` — passed; the
  Browser image/profile/runtime/bootstrap boundary and installed Workload
  launcher mapping were all present.
- Orchestrator tiny checks: `bash -n`, launcher self-test, and
  `git diff --check` — passed before farm dispatch.

Two broader pre-existing gates were inspected but are not claimed: the full
Browser contract currently expects the no-argument image builder to report the
missing-RPM error before its required frozen-profile error, and the activation
gate observed a concurrent out-of-scope `mackesd` Browser edit. The retired-host
Browser boundary also reports two existing signatures outside this slice. None
of those paths was edited here.

## Residual

This is source/package readiness integration, not a generated image or live
acceptance claim. The first release still must build the signed Lighthouse RPM
and reproducible Browser qcow2, atomically promote their admitted generation,
and then perform the deferred guest audio, reconnect, performance, upgrade, and
one-node acceptance against that exact image digest.
