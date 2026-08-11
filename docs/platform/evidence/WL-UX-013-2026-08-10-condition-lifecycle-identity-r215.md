# WL-UX-013 — condition lifecycle identity admission (r215)

- Scope: a condition identity cannot appear in both active and resolved
  lifecycle lanes within one health publication.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-condition-lifecycle-identity-r215 install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib health::tests::node_health_publication_rejects_condition_identity_split_across_lifecycle_lanes -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 517 filtered out`.
