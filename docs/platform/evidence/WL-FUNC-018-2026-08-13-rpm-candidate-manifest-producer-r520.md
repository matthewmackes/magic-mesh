# WL-FUNC-018 — governed App VM RPM candidate producer (r520)

## Result

The App VM local-image lane now has a canonical non-secret producer for one
exact, already-built and already-signed Workstation RPM. The schema-v2 manifest
binds all of the following before it can be published:

- whole-RPM SHA-256 and RPM payload SHA-256;
- canonical NEVRA, including a non-zero epoch when present;
- the exact non-null source revision re-attested from BuildInfo in both signed
  `mackesd` and `mde-shell-egui` payload members;
- the full primary or signing-subkey fingerprint resolved from the embedded RPM
  signature and the supplied governed release public key; and
- immutable App VM target `mcnf-app-vm/wayland-standard-v1`.

The existing embedded RPM signature is the governed release attestation. The
producer neither accepts nor handles a private-key path, does not build or
rewrite an RPM, and does not create a second trust store or detached signature.
It snapshots a single-link, bounded, non-writable RPM, self-verifies the staged
manifest through the production App VM supply verifier, then atomically
publishes a mode-0400 manifest in a new mode-0700 directory. Missing or
ambiguous signing authority, stale BuildInfo, source/RPM mutation, substituted
target/signer/NEVRA, symlinked input, unsafe output authority, and an existing
output all fail without replacing output.

## Files

- `packaging/app-vm/produce-rpm-candidate-manifest.py`
- `packaging/app-vm/test-produce-rpm-candidate-manifest.py`
- `packaging/app-vm/verify-rpm-supply.sh`

## Farm gates

- `.50`, `func018-rpm-candidate-integration-r520c`: hostile producer/verifier
  integration passed, including exact-byte, target, signer, NEVRA, BuildInfo,
  output no-replace, permission, and missing-authority refusals.
- `.90`, `func018-rpm-supply-selftest-r520`: the production App VM RPM supply
  verifier self-test passed with schema-v2 identity admission.
- `.50`, `func018-rpm-candidate-shellcheck-r520b`: ShellCheck passed.
- `.196`, `func018-rpm-candidate-pycompile-r520c`: both new Python programs
  compiled.
- Local `bash -n` and scoped `git diff --check`: passed.

The initial `.170` ShellCheck dispatch found no `shellcheck` executable and
claimed no result; the exact gate was rerouted to `.50`. An initial integration
fixture accidentally pre-created its stale-output path; the fixture was fixed
and the complete hostile suite rerun. Neither failed dispatch is acceptance
evidence.

## Remaining WL-FUNC-018 inputs

- Build and release-sign the real first-release `magic-mesh` Workstation RPM.
- Run this producer against that exact RPM, exact first-release revision, and
  governed public key; supply its immutable output to the App VM image build.
- Supply the already-governed catalog trust receipt/key and immutable App VM
  base image, then build and verify the first-release App VM image/package.
- After the first release, perform the deferred non-blocking one-node lifecycle,
  Flatpak launch, VDI, recovery, provider-loss, and visual acceptance proof.
