# WL-FUNC-019 service-action admission — r159

- Revision: `e1b785dd`
- Scope: service actions fail closed unless authenticated, resource-targeted, issued, unexpired, ready, and unambiguous.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func019-service-admission-r159b install-helpers/xcp-build.sh cargo test -p mde-shell-egui --bin mde-shell-egui chooser::resources::tests::expired_ready_service_action_is_not_admitted -- --nocapture`
- Result: `1 passed; 0 failed; 1542 filtered out` on seat 50.

