# WL-UX-013 — decommissioned health projection cleanup (r217)

- Scope: when a publisher leaves the approved roster, its retained health ledger, Bus cursor, projection, and restart checkpoint entry are evicted only after current roster sources stage successfully.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-health-roster-retention-r217-final install-helpers/xcp-build.sh cargo test -p mackesd --features async-services workers::health_reconciler::tests::health_ingress_evicts_decommissioned_projection_and_checkpoint_on_restart -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4745 filtered out; finished in 0.02s`.
