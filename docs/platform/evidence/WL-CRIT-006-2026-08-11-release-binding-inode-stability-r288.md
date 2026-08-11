# WL-CRIT-006 release-binding inode stability evidence — 2026-08-11

- Scope: `ci-gate.sh bind-release` opens the caller's release descriptor once,
  hashes that descriptor before canonicalization, and performs every schema and
  binding read through the stable file descriptor.
- Hostile boundaries: pathname replacement cannot switch the authenticated
  input after validation; mutation of the same inode between the initial digest
  and canonicalization also fails closed. Rejected inputs preserve both gate
  status and authenticated log bytes.
- Gate: `bash install-helpers/ci-gate.sh --self-test`.
- Farm: `172.20.0.50`, slot 2.
- Result: **PASS** — the self-test reported policy failures, authenticated
  status, and fail-closed release binding all propagate.
- Infrastructure note: earlier BigBoy and `.170` attempts stopped before the
  binding fixtures because those images lacked `jq`; neither was counted as a
  code result. The `.50` image supplied the required tool.
- Remaining boundary: a production release still needs the complete signed
  current-revision CI/package/topology/seat/lighthouse/recovery bundle.
