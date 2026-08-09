# WL-CRIT-006 governed candidate path r5 — 2026-08-09

This advances S1-S3 packaging and evidence integrity; it does not claim a
current candidate, production acceptance, deployment, or signature.

## Current-revision exercise

BigBoy `172.20.0.130`, slot `crit006-candidate-r5`, received a clean detached
worktree at revision `b70e658bf8bc0a0677c934a5d891c31762ebeecd`. The governed
`xcp-build.sh rpm` path refused before compilation because that immutable
revision's `Cargo.lock` omitted the `jiff` dependency required by `mackesd`.
`--locked` was not relaxed and no partial RPM was represented as a candidate.

The same revision's lighthouse manifest also omitted the resource-publisher
credential helper, drop-in, and `mcnf-resource-publisher-credential.service`.
An older real lighthouse RPM was rejected for those exact three missing paths.

## Correction and focused proof

- The lighthouse package now declares all three credential payload assets.
- `write-candidate-manifest.py` snapshots each single-link RPM once through an
  `O_NOFOLLOW` descriptor, loops until every byte is copied, and re-hashes the
  private snapshot after header, payload-list, and runtime-binary inspection.
- It accepts only this repository's 40-hex Git object IDs and refuses tracked or
  ordinary untracked checkout changes. Its manifest is accepted by the exact
  `collect-six-node-topology.py` schema parser; the separate source receipt has
  one schema field and binds role, RPM bytes, RPM payload digest, and runtime
  binary digests.
- The authoritative GitHub farm job now emits both documents from final RPM
  bytes. The branch-protection job regenerates and byte-compares both documents
  before accepting the candidate.

BigBoy focused gates passed: Python compilation, writer hostile self-test
(64-hex refusal, dirty checkout refusal, short-write-safe immutable snapshot,
snapshot mutation refusal, unique receipt fields, exact/hostile schema), current
base+lighthouse credential payload gate, and workflow YAML parse. The original
manifest fixture failed the new lighthouse payload gate as required.

## Exact operator/signing blocker

The correction is uncommitted, so it cannot identify itself as an immutable
source candidate. Commit it together with the independently owned `Cargo.lock`
repair, then let the governed GitHub required workflow build final base and
lighthouse RPMs and emit the machine-readable candidate documents for that same
revision. Production signing remains blocked until that run is green and a
fresh governed `resource-publisher-hmac` credential/attestation is available;
the orchestrator cannot read that credential. A local Magic Mesh release secret
key exists, but it was not used and no signature was fabricated.
