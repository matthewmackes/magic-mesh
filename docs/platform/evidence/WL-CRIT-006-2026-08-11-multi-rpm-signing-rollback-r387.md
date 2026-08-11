# WL-CRIT-006 multi-RPM signing rollback — 2026-08-11

- Scope: batch RPM signing retains private rollback copies until every artifact signs and verifies successfully.
- Hostile boundary: failure after mutating the second RPM restores every artifact to its original hash and leaves no backup residue.
- Focused gate: sync through `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh sync`, then run `install-helpers/sign-release.sh --self-test` in that farm workspace.
- Farm: `172.20.0.90`, slot 2, admitted with 23,029,388 KiB free.
- Result: **PASS**, focused signing self-test including the hostile second-artifact failure assertion.
- Remaining boundary: actual governed operator key/keyring and trusted publisher credential proof remain.
