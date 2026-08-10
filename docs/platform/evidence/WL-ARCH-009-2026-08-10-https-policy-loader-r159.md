# WL-ARCH-009 HTTPS fallback policy loader — r159

- Revision: `8c72ab1d`
- Scope: HTTPS fallback configuration uses the environment first, then a bounded descriptor-anchored policy file; oversized, malformed, symlinked, invalid-host, and port-zero values fail closed.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-https-policy-r159 install-helpers/xcp-build.sh cargo test -p mackesd --lib transport::https443::tests::policy_file_loader_is_bounded_and_fail_closed -- --nocapture`
- Result: `1 passed; 0 failed; 4699 filtered out` on BigBoy.

