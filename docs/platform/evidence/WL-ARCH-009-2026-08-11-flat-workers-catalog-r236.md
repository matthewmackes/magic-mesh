# WL-ARCH-009 flat Workers catalog — 2026-08-11

- Scope: `Surface::Workers` owns one deterministic leaf-only catalog for node,
  fleet, network, discovery, phone, provisioning, and Action Console views.
  Provider views no longer add a nested route rail or tab strip.
- Farm: BigBoy `172.20.0.130`, slot `1`.
- Compile: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh cargo check -p mde-shell-egui`.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh cargo test -p mde-shell-egui workers_catalog::tests::catalog_is_unique_and_deterministically_sorted -- --exact`.
- Result: PASS, 1 passed, 0 failed, 1,547 filtered out. The complete shell
  compile also passed; existing warning debt remained warnings only.
