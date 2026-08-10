# WL-ARCH-009 explicit worker aliases — r158

- Revision: `7012bf93`
- Scope: runtime worker aliases are registry-owned; unknown normalized aliases fail closed.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-worker-alias-r158 install-helpers/xcp-build.sh cargo test -p mackesd --lib worker_role::tests::runtime_aliases_are_explicit_and_unknown_normalizations_fail_closed -- --nocapture`
- Result: `1 passed; 0 failed; 4698 filtered out` on seat 90.

