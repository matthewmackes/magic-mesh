# WL-FUNC-019 — ambiguous peer identity isolation (r207)

- Scope: divergent peer rows claiming one hostname cannot authorize downstream
  Workload, App, or Android resource reads; exact duplicate rows remain valid.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func019-peer-identity-isolation-r207 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::service_aggregator::resource_adapters::tests::ambiguous_peer_identity_cannot_authorize_downstream_resource_reads -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4736 filtered out`.
