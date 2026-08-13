# WL-FUNC-022 shell full coding suite — 2026-08-13

- Scope: full Construct shell binary test suite covering the unified Workers surface, Clock chrome, navigation, VDI boundaries, accessibility, and UI routing.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func022-shell-full-rerun-20260813 install-helpers/xcp-build.sh cargo test -p mde-shell-egui --locked --bin mde-shell-egui`.
- Result: **PASS**, 1,581 passed, 0 failed, 0 ignored; seat `.50`.
- This is a coding-release gate. Post-release live/package proof remains explicitly non-blocking per the canonical worklist policy.
