# WL-CRIT-007 recovery role admission r3

- Scope: deterministic peer-recovery fail-closed fixture; no physical sleep claim.
- Gap closed: an existing but malformed or unsupported `role.toml` could enter
  network and service recovery under ambiguous node identity.
- Change: `mesh-peer-recovery.sh` now admits exactly one `workstation` or `lighthouse`
  before checking network state, taking the recovery lock, or mutating services.
- Host/slot: BigBoy `172.20.0.130`, `crit007-role-admission-20260809-r3`.
- Verification: `bash -n` passed for the helper and fixture;
  `install-helpers/test-mesh-peer-recovery.sh` passed, including
  hostile unsupported-role, malformed-quote, and duplicate-role fixtures proving an empty
  mutation ledger.
