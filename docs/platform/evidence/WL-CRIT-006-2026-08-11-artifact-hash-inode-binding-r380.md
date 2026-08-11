# WL-CRIT-006 artifact hash inode binding — 2026-08-11

- Scope: release artifact validation hashes one opened inode and rechecks pathname identity and metadata after hashing.
- Hostile boundary: same-sized inode replacement immediately after hashing fails closed instead of authenticating substituted bytes.
- Focused gate: sync through `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh sync`, then run `install-helpers/release-evidence.sh --self-test` in that farm workspace.
- Farm: BigBoy `172.20.0.130`, slot 1, admitted with 16,886,844 KiB free.
- Result: **PASS**, one focused self-test invocation including the hostile replacement assertion.
- Remaining boundary: operator-gated signing and trusted publisher-key verification remain.
