# WL-ARCH-008 — immutable Browser VM Lighthouse RPM handoff (r519)

Date: 2026-08-13

## Gap closed

The Browser VM producer defaulted to installing the repository's current
`magic-mesh-lighthouse` package even though the image advertised an immutable
Browser source revision. A fresh release could therefore combine the Browser
runtime with guest control-plane bytes from a different build, and neither the
OCI identity nor the guest filesystem recorded which RPM was admitted.

The producer now requires exactly one regular, non-symlink Lighthouse RPM. It
verifies the package name, hashes the file before the container build,
re-attests the exact digest inside the Containerfile before `dnf` can install
it, and publishes that digest in both the OCI label and immutable guest
metadata. Image verification requires the two records to agree. There is no
repository-latest fallback.

The hostile self-test removes the in-image `sha256sum --check --strict`
re-attestation and proves that the contract rejects the modified producer. It
also proves that invoking the production builder without an immutable RPM is
rejected before Podman or image mutation.

## Farm gates

- `.130`, slot `arch008-lighthouse-contract-r519`:
  `packaging/browser-vm/verify-contract.sh --lighthouse-rpm-self-test` — passed.
- `.50`, slot `arch008-lighthouse-shellcheck-r519`:
  `shellcheck -e SC2119,SC2120 packaging/browser-vm/build-image.sh packaging/browser-vm/verify-image.sh packaging/browser-vm/verify-contract.sh` — passed. The two excluded findings are the pre-existing argument-forwarding warnings in the untouched legacy `run_validator` fixture.
- Local `bash -n` and scoped `git diff --check` — passed before dispatch.

The complete `verify-contract.sh` was also attempted on `.130`. Its owned
packaging checks ran, then the independent activation contract rejected a
concurrent change because `browser.rs` no longer contains the old textual
`BrowserVmProfile::default().workload_spec(node, name)` pattern. That broad
gate is not claimed here and no out-of-scope Workloads source was changed.

No Browser image, RPM, or release was built, and no live proof was attempted.
