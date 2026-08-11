# WL-ARCH-009 — HTTPS policy source-parent integrity (r214)

- Scope: the HTTPS policy loader rejects a symlinked parent without reading
  configuration outside the governed policy root.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-https-policy-parent-r214 install-helpers/xcp-build.sh cargo test -p mackesd --lib --features async-services transport::https443::tests::policy_file_loader_rejects_a_symlinked_parent_without_reading_outside -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4741 filtered out`.
