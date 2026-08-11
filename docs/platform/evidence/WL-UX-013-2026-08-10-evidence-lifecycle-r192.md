# WL-UX-013 — health evidence lifecycle ordering (r192)

- Scope: expected-state/history contract admission.
- Runtime: `crates/mesh/mackes-mesh-types/src/health.rs`.
- Change: a condition is rejected when its evidence timestamp predates
  `active_since_ms` or is newer than `last_observed_ms`. Such records describe
  an impossible lifecycle and must not reach the health projection or history.
- Farm gate:

  ```text
  MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-evidence-lifecycle-r192 \
    install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib \
    health::tests::node_health_publication_rejects_secrets_oversized_evidence_and_bad_lifecycle \
    -- --exact --nocapture
  ```

- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 516 filtered out`.
- Local `git diff --check`: passed.
- Live-proof limit: this is a shared contract/admission checkpoint; no live
  three-seat health modal or physical recovery transition was exercised.
