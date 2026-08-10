# WL-UX-013 duplicate active-condition identity — r185

- Scope: a node-health publication must not carry the same active condition identity more than once; duplicate active rows can otherwise distort history/provenance even when grade calculation deduplicates them.
- Change: `NodeHealthState::validate_at` rejects duplicate `(scope, id)` identities in `active_conditions`. Repeated resolved records remain admitted for recurrence aggregation.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-duplicate-active-condition-r185 install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib health::tests::node_health_publication_rejects_duplicate_active_condition_identity -- --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 516 filtered out` on seat `.90`.
- Live-proof limit: this is a contract/admission gate; no live three-seat health modal or physical recovery transition was exercised.
