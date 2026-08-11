# WL-CRIT-006 mandatory release-signing evidence — 2026-08-11

- Scope: release signing now has two explicit operator-only stages. First,
  `--prepare-rpms` validates the complete same-directory RPM set, embeds and
  verifies RPM signatures, and emits no provenance, checksums, or detached
  release signature. Candidate finalization consumes only those prepared RPMs
  and independently verifies their governed signatures. Later, `--evidence`
  final publication re-verifies each embedded RPM signature and the exact
  evidence-bound bytes without mutation. Artifacts, evidence, SBOM, and gate
  manifests remain bound to the validated inode. Canonical provenance and
  checksums are built and fsynced on private inodes; GPG signs the already-open
  checksum inode, then Linux atomic no-replace renames publish the exact
  provenance/checksum/signature inodes. Identity and checksum digest are checked
  before and after signing/publication. Unsigned RPMs, evidence-free invocation,
  source replacement, pre-existing output, and hostile output symlinks fail
  without a successful publication or GPG claim.
- Production path: operator RPM preparation → Surface candidate finalization →
  release-evidence capture → immutable final publication → detached GPG
  signature.
- Focused gates passed locally:
  - `bash -n install-helpers/sign-release.sh`;
  - `install-helpers/sign-release.sh --self-test` (complete-set preflight,
    mutation isolation, unsigned-RPM refusal, inode replacement, hostile
    `SHA256SUMS` symlink insertion, exact GPG input, and atomic no-replace
    publication);
  - `install-helpers/finalize-surface-stack.py --self-test` (15 hostile
    fixtures rejected, including an output appearing during finalization and a
    producer RPM replaced after snapshot validation; Linux atomic no-replace
    publication preserved both identities).
- The finalizer copies every producer directory, prepared RPM, source bundle,
  release key, and certificate from a stable opened inode into a private
  read-only snapshot. Verification, selection, copying, and manifest hashing
  consume only those snapshots. Immediately before publication it rechecks the
  original inode metadata, bytes, and exact directory membership, refusing a
  changed input without emitting a candidate.
  - targeted `git diff --check`.
- These are tiny helper self-tests/source gates and did not warrant a farm
  build.
