# WL-CRIT-006 — RPM signing identity receipt (r520)

Date: 2026-08-13

The canonical first-release RPM path now consumes a bounded, non-secret signing
identity receipt instead of trusting a caller-supplied fingerprint. The producer
uses the existing `MAGIC_MESH_SIGN_KEY`/`Magic Mesh Release Signing` GPG
configuration, requires exactly one primary secret-key fingerprint, and admits
it only when it equals the primary fingerprint in the tracked
`packaging/repo/RPM-GPG-KEY-magic-mesh` trust root. It emits only public identity
metadata: configured identity label, primary fingerprint, public-key SHA-256,
exact source revision, and exact commit epoch. It never invokes key generation,
secret-key export, RPM signing, or a release build.

Production writes are canonical JSON bounded to 4 KiB, mode `0400`, fsynced,
and atomically published with no replacement. Inspection reopens a regular
non-symlink receipt with identity/size/change checks, requires canonical schema,
re-resolves the currently configured signing identity, rechecks the governed
public key, and requires exact revision and epoch equality. Missing, ambiguous,
foreign, stale, malformed, replaced, non-canonical, or mismatched input fails
before the canonical RPM path synchronizes source or performs build/signing
mutation.

## Exact farm gates

- `.50`, slot `crit006-rpm-identity-hostile-r4`:
  `python3 install-helpers/test-produce-rpm-signing-identity-receipt.py` passed,
  followed by `install-helpers/test-release-input-preflight.sh` passing its valid
  fixture and hostile missing/mismatched/null preflight fixtures.
- `.170`, slot `crit006-rpm-identity-compile-r4`:
  `python3 -m py_compile install-helpers/produce-rpm-signing-identity-receipt.py install-helpers/test-produce-rpm-signing-identity-receipt.py`
  passed.
- `.50`, slot `crit006-rpm-identity-shellcheck-r4`:
  `shellcheck -e SC2016 install-helpers/release-input-preflight.sh install-helpers/test-release-input-preflight.sh install-helpers/xcp-build.sh`
  passed. `SC2016` excludes only the two pre-existing intentional remote-literal
  findings in untouched `xcp-build.sh` lines 87 and 532.
- `.90`, slot `crit006-rpm-identity-bash-r4`:
  `bash -n install-helpers/release-input-preflight.sh install-helpers/test-release-input-preflight.sh install-helpers/xcp-build.sh`
  passed.
- Local `git diff --check` passed. No release, RPM build, key generation,
  private-key export, or signing mutation ran.

## Remaining WL-CRIT-006 first-release inputs

- Supply the real UX-014 A–F package assets admitted by the Kiron verifier.
- Supply the governed App VM catalog trust receipt/key and immutable App VM base
  image digest.
- Supply the signed Cuttlefish declaration, readiness relay, VDI agent, declared
  guest packages, and immutable Cuttlefish image digest.
- Supply the immutable bootc base-image digest.
- On the authorized release machine, produce this RPM signing identity receipt
  for the exact clean source revision and its commit epoch.
- Run the first full release build, then verify generated RPM payloads,
  signatures, release evidence, and artifact integrity.
- After release, perform the deferred non-blocking one-node installed acceptance,
  recovery, and corrected-forward proof required by WL-CRIT-006.
