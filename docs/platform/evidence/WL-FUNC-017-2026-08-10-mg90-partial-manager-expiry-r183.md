# WL-FUNC-017 MG90 partial-manager expiry — r183

- Scope: expiring one manager assignment no longer clears a source-level publication while another approved manager still has a live snapshot.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func017-mg90-partial-expiry-r183b install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::vehicle::tests::expiring_one_manager_preserves_live_source_publication_epoch -- --exact --nocapture`.
- Result: `1 passed; 0 failed; 4716 filtered out` on seat `.90`; the regression preserves the healthy manager's publication clock during partial failover.
