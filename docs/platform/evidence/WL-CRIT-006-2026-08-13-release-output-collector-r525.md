# WL-CRIT-006 — immutable first-release output collection (r525)

Date: 2026-08-13

`install-helpers/collect-release-outputs.py` is the canonical post-build
collector for the first complete release. It performs no build, signing,
publication, promotion, or placeholder creation. It accepts exactly one of each
required output role: Workstation RPM, Server RPM, Lighthouse RPM, Browser VM,
App VM, Cuttlefish image, and bootc image.

Every output must be a bounded regular non-symlink file and must pass its owning
verifier with the exact artifact path and source revision. RPM verifiers also
receive the exact governed signing identity. The collector rejects missing or
duplicate roles, duplicate paths/inodes, null or malformed revision/signing
identity, wrong media type or file magic, verifier refusal, unbounded verifier
output, and file replacement or mutation across verification and measurement.
It then records canonical SHA-256, byte size, media type, source revision, and
signing identity.

The owning verifier must itself be an executable tracked file from the exact
pinned source checkout. The collector rejects external executables, untracked
helpers, a checkout whose `HEAD` differs from the release revision, non-executable
Git modes, and verifier bytes modified after that revision. This prevents a plan
from replacing an owning verifier with `/bin/true` or another detached helper.

The final JSON is bounded to 1 MiB, canonicalized, fsynced, mode `0400`, and
published with same-filesystem exclusive-link/no-replace semantics. Its
`promotion` field is permanently `forbidden`. The collector is downstream of,
and does not replace, any owning artifact verifier.

## Farm evidence

- `.50`, slot `crit006-output-hostile-r525`:
  `python3 install-helpers/test-collect-release-outputs.py` passed. The hostile
  suite covered the complete seven-role positive collection plus duplicate
  role, missing role, stale revision, null signer, verifier without artifact
  binding, RPM verifier without signer binding, wrong media type, duplicate
  inode/path, owning-verifier refusal, and attempted output replacement.
- `.170`, slot `crit006-output-compile-r525`:
  `python3 -m py_compile install-helpers/collect-release-outputs.py install-helpers/test-collect-release-outputs.py`
  passed.
- `.196`, slot `crit006-output-tabnanny-r525`:
  `python3 -m tabnanny install-helpers/collect-release-outputs.py install-helpers/test-collect-release-outputs.py`
  passed.
- `.50`, slot `crit006-output-verifier-r526`:
  the expanded hostile suite passed, including external `/bin/true` substitution
  and a tracked verifier modified after the pinned revision.
- `.170`, slot `crit006-output-static-r526`:
  Python compilation and tabnanny passed for the hardened collector and suite.
- Local `git diff --check` passed.

ShellCheck is not applicable: this slice contains only Python executables and
Markdown evidence. Python compilation and tabnanny are the corresponding static
language gates.

## Remaining WL-CRIT-006 acceptance

- Supply all real governed first-release inputs, including UX-014 assets,
  signing authority receipts, signed RPMs, and immutable derivative/base image
  receipts.
- Run the first full release build and invoke every owning verifier on its real
  output.
- Run this collector against those admitted outputs and retain the resulting
  immutable manifest with the release evidence.
- Verify package payloads, RPM headers/signatures, derivative manifests, and
  release artifact integrity.
- Perform deferred, non-blocking post-release one-node installed acceptance,
  recovery, and corrected-forward proof.
