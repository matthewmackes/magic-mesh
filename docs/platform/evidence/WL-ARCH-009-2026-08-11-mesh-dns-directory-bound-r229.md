# WL-ARCH-009 mesh-DNS directory bound — 2026-08-11

- Scope: replicated mesh-DNS directories over 12 peers fail closed before generating records or a managed hosts block.
- Farm: BigBoy `172.20.0.130`, slot `ux012-mesh-directory-bound-r229`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux012-mesh-directory-bound-r229 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::mesh_dns::tests::oversized_directory_fails_closed_without_mesh_records -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
