# WL-FUNC-018 — governed App VM catalog trust producer (r519)

Date: 2026-08-13

## Result

`install-helpers/produce-app-vm-catalog-trust.py` is the canonical first-release
input producer for the catalog trust receipt consumed by the App VM image build.
It admits the existing project OpenPGP release authority, requires the matching
secret authority to be available to the operator, and extracts the exact raw
32-byte Ed25519 verification key from the governed primary key's canonical
OpenPGP point. The receipt identifies that primary fingerprint, binds the exact
resolved Git commit, and binds the canonical key bytes by SHA-256.

The producer writes mode-0400 receipt/key files in a mode-0700 staging directory,
fsyncs them, re-admits the exact output through
`verify-app-vm-catalog-trust.py`, and publishes the pair with Linux
`renameat2(RENAME_NOREPLACE)`. It neither reads nor writes private-key bytes.
Missing or substituted signing authority fails with
`REFUSED[WL-FUNC-018/catalog-trust-producer]` before output publication.

No App VM image or release artifact was built.

## Farm gates

- `.50`, slot `func018-trust-producer-hostile-r519b`:
  `python3 install-helpers/test-produce-app-vm-catalog-trust.py` — passed. The
  suite covers success, exact revision/fingerprint/key derivation and modes,
  missing secret authority, mismatched authority, wrong algorithm, malformed
  point, unsafe parent, and existing-output preservation.
- `.170`, slot `func018-trust-producer-compile-r519b`:
  `python3 -m py_compile install-helpers/produce-app-vm-catalog-trust.py install-helpers/test-produce-app-vm-catalog-trust.py`
  — passed.
- `.196`, slot `func018-trust-producer-missing-authority-r519`:
  production producer against an isolated committed fixture and the farm
  account's real keyring — exited 2 with
  `REFUSED[WL-FUNC-018/catalog-trust-producer]: governed release secret signing authority lookup failed`
  and published no output — passed.
- Local orchestration-only `git diff --check` on the owned files — passed.

## Remaining WL-FUNC-018 inputs

- The release operator must run this producer with the governed secret signing
  authority available for the exact first-release source revision.
- The first release still needs the resulting receipt/key plus the signed App VM
  RPM candidate manifest and immutable base image input before the image build.
- Image/package verification belongs to the first full release build. Installed
  one-node App VM lifecycle, restart, loss, and visual proof remains deferred and
  non-blocking until after that release.
