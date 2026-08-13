# WL-FUNC-018 — governed App VM base-image receipt (r523)

Date: 2026-08-13

## Result

`packaging/app-vm/produce-base-image-receipt.py` is now the canonical producer
and inspector for the immutable App VM base-image input. Its no-replace,
mode-0400 JSON receipt binds the original registry reference, exact manifest or
index digest, unique Linux platform digest, architecture, registry media type,
`mcnf-app-vm/wayland-standard-v1` target, `wayland-standard` profile, exact
source revision, and matching commit epoch.

The producer performs a bounded manifest-only registry inspection through
Skopeo. It does not pull image layers, handle registry credentials, build an
image, or create placeholder artifacts. Duplicate JSON fields, malformed or
ambiguous platform entries, changed manifest bytes, source/epoch mismatch,
unsafe receipt files, and output replacement all fail closed.

`packaging/app-vm/build-image.sh` now requires that receipt. It repeats the
registry inspection and validates every binding before RPM staging, Podman
storage mutation, or image-context mutation. The `FROM` build argument is a
registry reference pinned to the admitted platform digest (or the admitted
single-manifest digest), replacing local image-ID authority while preserving
the original reference as provenance.

No image layer was pulled and no App VM image or release artifact was built.

## Farm gates

- `.50`, slot `func018-appbase-contract-r523`:
  `packaging/app-vm/verify-contract.sh --base-receipt-self-test` — passed. This
  runs the hostile producer/revalidation suite and verifies admission precedes
  the first RPM/image-context mutation.
- `.50`, same released slot:
  `shellcheck packaging/app-vm/build-image.sh packaging/app-vm/verify-contract.sh`
  — passed with no findings.
- `.50`, slot `func018-appbase-python-r523`:
  `python3 -m py_compile ... && python3 -m tabnanny ...` for the producer and
  hostile test — passed.
- `.196`, slot `func018-appbase-bash-r523`:
  `bash -n packaging/app-vm/build-image.sh packaging/app-vm/verify-contract.sh`
  — passed.
- Local orchestration-only `git diff --check` — passed.

The ShellCheck lane was initially attempted on `.90` and `.196`; neither host
had ShellCheck installed, so no result was claimed there. It was rerouted to
the released `.50` lane and passed.

## Remaining WL-FUNC-018 inputs

- Publish the real immutable App VM base manifest and produce this receipt for
  the exact first-release revision and architecture.
- Build and release-sign the real Workstation RPM, then produce its governed
  candidate manifest and the catalog trust receipt/key.
- Build, verify, package, and promote the first App VM image during the first
  full release.
- Installed one-node App VM lifecycle, Flatpak, VDI, restart, provider-loss,
  and visual proof remains deferred and non-blocking until after that release.
