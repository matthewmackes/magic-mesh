# WL-CRIT-006 — immutable bootc digest receipt r521

The first-release bootc input now has a canonical non-secret producer and
inspector at `install-helpers/produce-bootc-digest-receipt.py`. Production uses
one bounded `skopeo inspect --raw` registry read. The SHA-256 of those exact
manifest or manifest-list bytes is the admitted immutable digest. A list must
contain exactly one unqualified `linux/<architecture>` entry; a single manifest
requires a bounded config inspection proving the same platform.

The canonical JSON receipt binds the configured image reference, resolved
digest and media type, architecture, exact source revision, matching commit
epoch, and expected release role. Publication is mode 0400, fsynced, atomic,
and no-replace. Inspection rejects symlinks, oversized or non-canonical JSON,
schema drift, null/malformed digests, and any release-identity mismatch. No
registry credential, private material, image layer, build, pull, or release is
created.

## Farm evidence

- `.50`, slot `crit006-bootc-receipt-test-r521b`: hostile producer/inspector
  suite passed, covering list resolution, exact architecture selection,
  revision/epoch/role binding, tampering, symlinks, duplicate platforms,
  no-replace publication, and unavailable-registry refusal.
- `.170`, slot `crit006-bootc-receipt-py-r521b`: Python byte-compilation of the
  producer and hostile suite passed.
- `.196`, slot `crit006-bootc-receipt-refusal-r521`: a bounded real invocation
  with an unavailable inspector executable refused with exit 2, emitted the
  typed `bounded bootc manifest inspection is unavailable` diagnostic, and
  published no receipt.
- Local `git diff --check` passed for the owned files.

The malformed first `.196` dispatch expanded fixture variables before SSH and
is explicitly not claimed; the corrected quoted rerun above is the evidence.

## Remaining CRIT-006 release inputs

The canonical RPM preflight still accepts a raw bootc digest. Consuming this
receipt is indispensable to enforce architecture, revision, epoch, and role at
the build boundary, but that integration would require edits outside this
slice's authorized ownership: `install-helpers/release-input-preflight.sh`, its
self-test, and `install-helpers/xcp-build.sh`. Until that bounded integration is
authorized and landed, operators must produce the receipt for the exact first
release but the canonical cut does not yet enforce it.

The real registry image reference, available registry manifest, exact release
revision/epoch/role, App VM and Cuttlefish governed inputs, UX-014 assets, RPM
signing receipt, immutable image digests, and first full release build remain.
Installed one-node proof stays deferred and non-blocking until after release.
