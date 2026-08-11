# WL-CRIT-006 evidence inode stability — 2026-08-11

- Scope: release evidence validation opens one regular non-symlink input and
  performs every schema, artifact, topology, CI, VDI, and binding read through
  that stable descriptor. It records the opened device/inode and refuses if the
  caller-visible pathname no longer identifies that inode before success.
- Hostile boundary: a wrapped `jq` atomically replaces the evidence pathname
  after the validator's first successful read with another schema-correct,
  freshly rebound envelope. Validation fails and does not claim the replacement
  path was checked.
- Gate-manifest boundary: validation opens the descriptor's exact inode, records
  device/inode, size, nanosecond mtime/ctime, and SHA-256, and copies it into a
  private read-only snapshot. Recursive matrix/topology verification consumes
  only that snapshot. Snapshot and opened-source metadata/digests are rechecked
  after copying and again before success, while the caller pathname must still
  identify the opened inode. Hostile byte-identical inode replacement and
  same-inode mutation fixtures both fail closed; the mutation fixture also
  proves the private snapshot is removed.
- Commands:
  - `bash -n install-helpers/release-evidence.sh`
  - `git diff --check -- install-helpers/release-evidence.sh`
  - `install-helpers/release-evidence.sh --self-test`
- Result: PASS; syntax and diff gates passed, and the extended self-test reported
  the deterministic binding round-trip and fail-closed validation suite.
- Farm: none; this was a tiny local shell/verifier gate, and no farm sync was
  started while free hosts were below their governed reserve.
- Remaining proof: a production decision still requires the complete signed
  current-revision CI, package, topology, three-seat, lighthouse, and recovery
  bundle.
