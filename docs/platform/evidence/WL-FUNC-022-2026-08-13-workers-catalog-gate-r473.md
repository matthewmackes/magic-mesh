# WL-FUNC-022 Workers catalog gate — 2026-08-13

- Scope: the unified Workers workspace's flat, leaf-only catalog remains unique, deterministically sorted, and keeps the canonical This Node destination.
- Test gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func022-workers-catalog-20260813 install-helpers/xcp-build.sh cargo test -p mde-shell-egui --locked workers_catalog -- --nocapture`.
- Result: **PASS**, 1 passed, 0 failed; seat `.50`.
- Clippy gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func022-workers-catalog-clippy-20260813 install-helpers/xcp-build.sh cargo clippy -p mde-shell-egui --locked --bin mde-shell-egui --tests` exited 0 with 3 existing warnings.
- This confirms the UI-side catalog boundary; post-release live/package proof remains explicitly non-blocking.
