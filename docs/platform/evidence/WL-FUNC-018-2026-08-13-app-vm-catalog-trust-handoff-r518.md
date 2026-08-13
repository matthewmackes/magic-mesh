# WL-FUNC-018 governed App-VM catalog trust handoff — 2026-08-13

- First-release gap closed: the production App-VM image driver now requires one
  bounded release-input receipt and one Ed25519 catalog verification key before
  RPM staging, registry access, or Podman mutation. The receipt binds the exact
  signer ID, the non-null 40-character release source revision, and the SHA-256
  of the canonical 32-byte public key.
- Admission opens authority inputs without following symlinks, requires one
  bounded singly-linked regular file with no group/world write authority,
  detects read-time identity changes, rejects duplicate JSON fields, and
  refuses missing, mutable, replaced, mismatched, or malformed material.
- Staging is bound to a private caller-owned directory FD. Publication uses
  no-follow relative opens and no-replace hard links, with named-path-to-FD
  device/inode checks before and after each output. A substituted or symlinked
  stage fails closed.
- The build receives only those private staged bytes as required Podman secrets.
  Its first trust operation revalidates receipt identity, release revision,
  signer, canonical key bytes, and digest before installing the key and receipt
  into the image. No alternate builder or trust store was introduced.
- Hostile production-driver coverage substitutes a fake Podman mutation
  sentinel and a mismatched receipt; the driver exits `2` and the sentinel is
  never invoked.

## Farm gates

- BigBoy `172.20.0.130`, slot `func018-trust-contract-r518`:
  `packaging/app-vm/verify-contract.sh` passed, including existing image/RPM
  provenance checks, the complete trust hostile suite, and the pre-Podman
  refusal sentinel.
- `.50` `172.20.0.50`, slot `func018-trust-shellcheck-r518`:
  `bash -n packaging/app-vm/build-image.sh packaging/app-vm/verify-contract.sh`
  and ShellCheck on both production shell files passed.
- `.196` `172.20.0.196`, slot `func018-trust-python-r517`:
  Python byte-compilation and
  `install-helpers/verify-app-vm-catalog-trust.py --self-test` passed, including
  duplicate-field, missing, mutable, replaced, revision/digest mismatch,
  symlink, stage substitution, and pinned-publication cases.
- Repository-aware local `git diff --check` passed. The farm synchronization
  intentionally omits `.git`, so no farm Git-hygiene result is claimed.

No image or release was built. Remaining FUNC-018 acceptance is to supply the
real governed first-release receipt/key and current RPM/base inputs, produce and
package the immutable App-VM image digest, then perform the deferred
non-blocking one-seat Flatpak/VDI sandbox, readiness, persistence, reconnect,
crash, stop, cleanup, and visual/audio proof after the first release.
