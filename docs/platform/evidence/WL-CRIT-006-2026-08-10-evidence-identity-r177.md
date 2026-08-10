# WL-CRIT-006 release evidence identity — r177

- Scope: release-gate verification binds every evidence filename to its gate scope and rejects cross-wired seat claims.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=crit006-evidence-identity-r177 install-helpers/xcp-build.sh sync`, followed on the synced farm workspace by `python3 install-helpers/verify-release-gate-matrix.py --self-test`.
- Result: `self-test PASS (1 valid, 16 hostile fixtures rejected)` on seat `.90`; no physical seats were used.
