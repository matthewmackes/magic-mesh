# WL-ARCH-008 — Browser base-image receipt (r522)

Date: 2026-08-13

## Result

The Browser image builder no longer treats a locally reported raw image digest
as sufficient base-image authority.  A canonical Browser-specific producer now
resolves exactly one registry reference without pulling layers and publishes a
mode-0400, no-replace JSON receipt.  It binds the top-level SHA-256
manifest/list digest and media type to the Linux architecture, the
`mcnf-browser-vm/browser-vm-chromium-v1` target, the
`browser-vm-chromium` profile, the exact source revision, and that revision's
commit epoch.  Multi-architecture indexes must contain exactly one matching
platform digest.

`build-image.sh` requires this receipt, recomputes the source epoch, and performs
a fresh bounded registry inspection.  A changed registry document, wrong
architecture/profile/target/revision/epoch, malformed or replaced receipt, or
ambiguous platform entry fails before the builder creates or changes its RPM
staging directory.  The producer and admission path do not pull layers, build
an image, accept credentials, or publish placeholders.

## Exact gates

- `.50`, slot `arch008-base-receipt-test-r522`:
  `packaging/browser-vm/verify-contract.sh --base-receipt-self-test` — passed,
  including hostile producer and mutation-order integration.
- `.170`, slot `arch008-base-receipt-python-r522`:
  `python3 -m py_compile` and `python3 -m tabnanny` over both receipt scripts —
  passed.
- `.196`, slot `arch008-base-receipt-shell-r522`:
  `bash -n packaging/browser-vm/build-image.sh packaging/browser-vm/verify-contract.sh`
  — passed.  This host did not contain ShellCheck, so no ShellCheck result is
  claimed from it.
- `.50`, same released slot after the integration gate:
  `shellcheck -e SC2119,SC2120 packaging/browser-vm/build-image.sh packaging/browser-vm/verify-contract.sh`
  — passed.  The two exclusions are established findings in the untouched
  legacy `run_validator` fixture block; this patch introduced neither class.
- Local `git diff --check` and syntax probes — passed.

## Remaining ARCH-008 acceptance

- Publish the real Browser base registry manifest and produce its receipt for
  the exact first-release source revision.
- Build and release-sign the real Lighthouse RPM and produce its governed
  candidate manifest.
- Build, verify, package, and promote the resulting immutable Browser image in
  the first full release.
- Complete the separately owned alternate-transport implementation.
- After release, perform the deferred non-blocking one-node VDI, audio,
  migration, reconnect, and performance acceptance.
