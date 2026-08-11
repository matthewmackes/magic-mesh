# WL-FUNC-020 — stale Android generation admission (r210)

- Scope: Cuttlefish inventory and launch operations refuse stale or non-ready
  generation-specific state before backend contact.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func020-stale-generation-admission-r210 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::cloud::verbs::android::cuttlefish::tests::stale_generation_operations_stop_before_backend_contact -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4739 filtered out`.
