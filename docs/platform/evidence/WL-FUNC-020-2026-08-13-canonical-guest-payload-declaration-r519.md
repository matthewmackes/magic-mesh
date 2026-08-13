# WL-FUNC-020 canonical guest-payload declaration producer — 2026-08-13

## Scope

The first-release Cuttlefish payload now has one canonical producer:
`packaging/android/produce-guest-payload-declaration.sh`. It hashes the exact
single-link regular readiness relay, VDI agent, and ordered non-empty package
set; binds the full source Git object ID, closed provider identity, immutable
image ID and digest, release ID, and compatibility version; and signs the
canonical JSON with the existing Magic Mesh release authority.

The production consumer previously admitted an exact schema-v1 object that had
no fields for source, provider, or image identity. The indispensable coupled
change advances `verify-guest-payload.sh` to exact schema v2 so it consumes the
new signed bindings while retaining the fixed installed public key and governed
primary fingerprint.

The producer has no private-key path option and creates no key or trust store.
Production resolves only `MAGIC_MESH_SIGN_KEY` (or the existing
`Magic Mesh Release Signing` default) in the operator keyring, requires primary
fingerprint `B546CC2EF9489F1899657AC9E6C820DAFBD1B07A`, and verifies the detached
signature before publication. Missing artifacts, a missing/wrong key, malformed
identity, aliased input, or an existing output fail before publication. A
candidate is published as a new directory with Linux `RENAME_NOREPLACE`; no
existing declaration or signature is replaced.

No Cuttlefish image or guest was built or started.

## Farm evidence

- `.50`, slot `func020-declaration-integration-r519`:
  `./packaging/android/produce-guest-payload-declaration.sh --self-test`
  passed. The hostile integration generated an ephemeral signing fixture,
  produced schema v2, consumed it through the production verifier function,
  staged both packages and both agents, and rejected output replacement,
  missing artifacts, missing signing authority, and signed source/provider/image
  substitutions.
- `.50`, slot `func020-declaration-shellcheck-r519`:
  `shellcheck packaging/android/produce-guest-payload-declaration.sh packaging/android/verify-guest-payload.sh`
  passed with no findings.
- `.170`, slot `func020-declaration-verifier-r519`:
  `./packaging/android/verify-guest-payload.sh --self-test` passed the consumer's
  existing artifact-substitution, hard-link, signature-tamper, and no-stage-on-
  refusal suite on schema v2.
- Local orchestration-only checks: `bash -n` and scoped `git diff --check`
  passed.

## Remaining first-release inputs

The real release still needs the already-built exact Cuttlefish `.deb` package
set, the release readiness-relay executable, the release VDI-agent executable,
the governed provider identity, the immutable Android image ID and SHA-256, the
release/compatibility IDs, and access to the existing project release private
authority on the operator signing machine. Those inputs can now be fed to this
producer without changing or bypassing the contract. First-release image
assembly must carry the resulting `release.json`, `release.json.asc`, and the
exact declared artifacts. Installed nested-KVM, readiness, VDI, launch,
restart, provider-loss, and visual proof remains deferred and non-blocking until
after the first release.
