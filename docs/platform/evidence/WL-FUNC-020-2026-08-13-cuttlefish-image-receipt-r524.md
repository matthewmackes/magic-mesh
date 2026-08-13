# WL-FUNC-020 — governed Cuttlefish image receipt (r524)

Date: 2026-08-13

## Result

The first-release Android path no longer admits a caller-supplied image digest.
`packaging/android/produce-image-receipt.py` produces and revalidates one
immutable receipt for either a registry manifest/index or a local Cuttlefish
image artifact. The receipt binds the exact digest and platform digest (when
applicable) to architecture, provider identity, Android release identity,
compatibility identity, source revision and commit epoch, media type/format,
source kind, and original source.

The signed guest-payload declaration now consumes only a successfully
revalidated receipt and embeds its exact canonical identity. The canonical RPM
release preflight revalidates the source before source synchronization or build
mutation, authenticates and stages the signed declaration, and requires the
declaration's image identity to equal the admitted receipt. Missing, stale,
substituted, cross-architecture, wrong-provider, wrong-release, wrong-revision,
hard-linked, symlinked, or ambiguous inputs fail closed.

No image was downloaded or built, no registry credential or signing private key
was handled, no placeholder was created, and no guest-runtime source or staging
role was changed.

## Farm evidence

- `.50`, slot `func020-image-contract-r524`:
  `packaging/android/verify-contract.sh` passed. This includes the hostile
  registry/artifact producer test, signed declaration producer/consumer path,
  payload verifier, and exact package contract.
- `.170`, slot `func020-image-preflight-r524`:
  `install-helpers/test-release-input-preflight.sh` passed, including refusal
  after the admitted artifact bytes were replaced.
- `.196`, slot `func020-image-python-r524`:
  `python3 -m py_compile` and `python3 -m tabnanny` passed for the producer and
  hostile test.
- `.50`, reused completed contract workspace:
  ShellCheck passed for every modified shell boundary with only the two
  established intentional `SC2016` remote-literal findings in untouched
  `install-helpers/xcp-build.sh` lines excluded.
- Local orchestration-only `bash -n` and `git diff --check` passed.

The `.90` and `.130` ShellCheck reroutes were not claimed because those hosts
do not have ShellCheck installed.

## Remaining FUNC-020 inputs

- Publish the real immutable Cuttlefish registry manifest or image artifact.
- Produce its receipt for the exact first-release revision, provider, Android
  release, compatibility identity, and architecture.
- Build the real guest packages, readiness relay, and VDI agent; produce the
  signed schema-v3 declaration over those exact bytes and the image receipt.
- Include and verify the admitted artifacts in the first full release.
- After release, perform the deferred non-blocking one-node nested-KVM,
  readiness, VDI, app-launch, restart, provider-loss, and visual acceptance.
