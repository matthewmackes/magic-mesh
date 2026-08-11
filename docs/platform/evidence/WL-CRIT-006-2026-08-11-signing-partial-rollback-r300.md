# WL-CRIT-006 signing partial-publication rollback evidence — 2026-08-11

- Scope: no-replace release publication tracks the exact device/inode of each
  output created by the signer.
- Hostile boundary: a later publication failure rolls back only those owned
  inodes. A replaced or hostile pathname is never removed; staging files and
  earlier signer-owned partial outputs are cleaned without provenance residue.
- Gates: `bash -n install-helpers/sign-release.sh` and
  `install-helpers/sign-release.sh --self-test`.
- Farm: BigBoy (`172.20.0.130`), slot 3.
- Result: **PASS** — syntax passed and the self-test reported RPM identity and
  atomic no-replace publication boundaries fail closed.
- Remaining boundary: complete signed current-revision CI/package/topology/
  seat/lighthouse/recovery publication remains open.
