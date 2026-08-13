# WL-CRIT-006 — canonical seven-role release-output plan producer (r529)

Date: 2026-08-13

## Result

`install-helpers/produce-release-output-plan.py` now translates one exact,
bounded release-input document into the schema consumed by
`collect-release-outputs.py`. The caller supplies artifact identities and
required companion files, but cannot supply or override verifier executable
paths, argv, media types, role names, substitutions, or repository roots.

The input schema contains exactly:

- schema/version identity, exact source revision and commit epoch;
- one governed RPM signing fingerprint and public key; and
- exactly one input object for each of Workstation RPM, Server RPM, Lighthouse
  RPM, Browser VM, App VM, Cuttlefish image, and bootc image.

The generated plan uses these repository-owned verification boundaries:

- Workstation: `packaging/app-vm/verify-rpm-supply.sh`, with the candidate
  manifest, governed key, exact revision, and explicit expected signer.
- Server: `packaging/server-rpm/produce-server-rpm-candidate.py reverify`, with
  the same four identity bindings.
- Lighthouse: `packaging/browser-vm/produce-lighthouse-rpm-candidate.py verify`,
  with the same four identity bindings.
- Browser VM: `packaging/browser-vm/verify-image-manifest.py verify`, with the
  generated frozen release-profile snapshot, image manifest, exact qcow2, and
  explicit source revision. The producer does not assume that tracked
  `profile.env` can self-identify the commit containing itself.
- App VM: `packaging/app-vm/verify-qcow2-manifest.py`, with exact qcow2,
  derivative manifest, and source revision.
- Cuttlefish: `packaging/android/produce-image-receipt.py inspect`, which
  rehashes the exact local image artifact and binds its receipt, architecture,
  provider, Android release, compatibility ID, format, media type, epoch, and
  source revision.
- bootc: `install-helpers/produce-bootc-digest-receipt.py inspect`. The immutable
  registry digest receipt is the collected local representation, carries the
  canonical `application/vnd.mcnf.bootc-image-receipt+json` media type, and is rebound
  to its exact image reference, architecture, release role, epoch, and revision.

Every caller-supplied file must be an absolute, non-empty, bounded,
single-link regular file that is not group/other writable. Owner-writable
tracked keys and normal `0644` release artifacts remain admissible. Files may not be duplicated across roles or
companions. JSON duplicate keys, unknown/missing fields, null identities,
transport-prefixed bootc references, unsupported Cuttlefish formats, symlinks,
group/other-writable inputs, and changes to any supplied file before plan
publication fail closed. The canonical plan is bounded to 1 MiB,
fsynced as mode `0400`, and published by an exclusive same-filesystem hard link
without replacing an existing output.

## Farm evidence

- `.50`, slot `crit006-output-plan-hostile-r529`:
  `python3 install-helpers/test-produce-release-output-plan.py` passed. The suite
  verifies every exact argv and companion mapping plus caller-supplied verifier
  injection, missing/extra roles, null/stale identities, malformed epoch,
  duplicate inode, relative/writable/symlink input, unsupported source-kind,
  duplicate JSON key, transport-prefixed bootc reference, and existing-output
  refusal.
- `.170`, slot `crit006-output-plan-compile-r529`:
  `python3 -m py_compile install-helpers/produce-release-output-plan.py install-helpers/test-produce-release-output-plan.py`
  passed.
- `.196`, slot `crit006-output-plan-tabnanny-r529`:
  `python3 -m tabnanny install-helpers/produce-release-output-plan.py install-helpers/test-produce-release-output-plan.py`
  passed.
- Local `git diff --check` passed. ShellCheck is not applicable because the two
  executables are Python.

## Remaining WL-CRIT-006 acceptance

- Finish the concurrent Browser frozen-profile producer and revision-aware
  verifier interface, then provide that exact snapshot to this producer.
- Supply the real signed RPMs, candidate manifests, immutable derivative/image
  outputs, receipts, release key, signer identity, and release revision/epoch.
- Generate this canonical plan, run the seven owning verifiers through the
  collector, and retain the resulting immutable release-output manifest.
- Run and verify the first full release build, RPM headers/signatures, payloads,
  derivative manifests, and release integrity.
- Complete deferred, non-blocking post-release one-node acceptance and recovery
  proof.
