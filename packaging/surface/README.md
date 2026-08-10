# Fedora 44 Surface stack provenance

`surface-stack.f44.json` is the fail-closed input contract for the five
Surface-specific packages in the one Workstation bootc image. It intentionally
records `blocked`/`unavailable`: no Fedora 44 repository or immutable package
evidence is currently asserted.

`surface-build-inputs.f44.json` is the earlier producer-side lock. It binds the
official linux-surface Fedora 43 packaging refs selected for the Fedora 44
rebuild, the Fedora kernel-ark input, the upstream libwacom tarball, the Surface
Secure Boot certificate, and a digest-pinned Fedora 44 builder image. It does
not claim that any RPM has been built or signed. Validate it or fetch the full
hash-checked source set into a new non-overwritten directory with:

```sh
install-helpers/fetch-surface-build-inputs.sh --self-test
install-helpers/fetch-surface-build-inputs.sh
install-helpers/fetch-surface-build-inputs.sh --output /path/to/new-source-bundle
```

The governed Fedora 44 userspace producer supports `iptsd`,
`libwacom-surface`, `surface-control`, and `surface-secureboot`. It consumes
only a newly fetched bundle, uses the digest-pinned Fedora builder, emits the
package's exact unsigned binary/source RPM set, and records hashes, the resolved
build-environment RPM inventory, and a build manifest in a new output
directory:

```sh
install-helpers/build-surface-userspace-f44.sh \
  --inputs /verified/input/path --output /new/rpm/path --package PACKAGE
```

Unsigned output is build evidence, not a deployable release artifact. Release
signing and the populated `surface-stack.f44.json` remain separate governed
steps.

The builder image and Surface sources are immutable, but the upstream package
helper currently resolves Fedora build dependencies from the live Fedora 44
repositories. The result is therefore bounded and hash-recorded, not a claim of
a hermetic or bit-for-bit reproducible build. Promotion must retain the emitted
artifact hashes and build-environment inventory and still pass the separate RPM
signature/provenance verifier.

The fetch step is deliberately separate from RPM construction and signing. The
kernel producer remains deliberately absent: the upstream Fedora helper embeds
its Secure Boot private key in the transient source RPM, and running that helper
inside a networked dependency-build container would expose the signer to
third-party build code. A governed replacement must build without any private
key, then sign the kernel and every module in a minimal network-disabled stage,
verify every signer, and rebuild `surface-secureboot` with that same public
certificate. Final RPMs additionally require the project release signing key.
Neither secret belongs in this repository, source bundle, SRPM, or networked
build stage.

The ready state authorizes a local, offline artifact bundle under
`packaging/surface/artifacts/`. Every ready contract must bind all of the
following:

- the exact `quay.io/fedora/fedora-bootc:44@sha256:...` base image digest;
- one local signing-key filename, its SHA-256, and its full fingerprint;
- local source-archive filename, upstream HTTPS URL, immutable commit or
  `refs/tags/...` ref, measured archive SHA-256, and SPDX license expression;
- exact local RPM filename, NEVRA, whole-file SHA-256, and full RPM signing-key
  fingerprint;
- for `kernel-surface` and `surface-secureboot`, the kernel/module signer string
  and DER certificate SHA-256. Userspace/data packages explicitly declare that
  kernel-module signing is not applicable and must not carry signer data.

Do not replace `null` with guessed values. Obtain the source archive, RPMs,
repository signing key, installed kernel/module signer, signing certificate,
and digest-pinned Fedora 44 bootc base from the governed build/publish lane.
Place only those key/RPM/source artifacts in `artifacts/`, calculate their exact
values, then change every row to `ready`. Partial readiness is refused.

For a ready contract, the verifier hashes every local file, queries every RPM's
NEVRA, verifies every signature against only the pinned local key, rejects extra
artifacts, and emits the short-lived install lock consumed by the Containerfile.
The image installs those exact RPM paths; it never imports a network key or
resolves Surface package names from a repository.

Validate the contract and hostile fixtures:

```sh
install-helpers/verify-surface-stack.sh --self-test
install-helpers/verify-surface-stack.sh
```

The second command currently exits `3` and prints `BLOCKED`; exit `0` means the
complete pinned manifest passed. Exit `1` means malformed or contradictory
evidence, and exit `2` is a verifier usage/runtime prerequisite error.
