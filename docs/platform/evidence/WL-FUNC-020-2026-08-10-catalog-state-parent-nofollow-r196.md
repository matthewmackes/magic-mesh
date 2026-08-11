# WL-FUNC-020 Android catalog state-parent no-follow admission — r196

- Scope: Android catalog cache replay and persistence now reject an existing
  symlink or non-directory anywhere in the state-file parent chain before
  cache reads or `create_dir_all`; the final state file retains no-follow
  admission as well.
- Hostile regression: a symlinked state parent is refused for both replay and
  replacement, while the real target catalog remains unchanged and readable.
- Farm gate:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func020-catalog-state-parent-nofollow-r196b install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::android_catalog::tests::catalog_cache_rejects_a_symlinked_state_parent -- --exact --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4730 filtered out` on
  seat `.90`.
- Live limits: no nested-KVM Android boot, installed guest package, VDI
  attachment, or physical-seat acceptance was exercised.
