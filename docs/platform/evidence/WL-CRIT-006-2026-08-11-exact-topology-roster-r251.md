# WL-CRIT-006 exact production topology roster — 2026-08-11

- Scope: production-pass schema-5 release evidence now cross-binds the verified
  six-node topology to the canonical gate manifest. Workstation IDs must match
  exactly `dell`, `seat15`, and `surface`; lighthouse IDs must match the three
  manifest lighthouses. Six structurally valid live nodes with substituted
  identities fail closed.
- Production path: release-evidence write/validate → gate-manifest verification
  → six-node topology verification → exact role-roster binding → schema-5
  publication.
- Focused gates passed locally:
  - `bash -n install-helpers/release-evidence.sh`;
  - `install-helpers/release-evidence.sh --self-test`, including the wrong-roster
    production-pass refusal;
  - targeted `git diff --check`.
- Remaining epic boundary: collect and publish a current-revision live bundle
  from those exact six governed nodes; this checkpoint does not claim that live
  acceptance.
