# WL-FUNC-019 evidence — service action schema admission (r220)

- Scope: typed service-onboard action ingress.
- Change: future schema versions, blank/oversized correlation IDs, and IDs
  containing path/control separators are rejected before capability targeting.
- Farm host: `172.20.0.50`.
- Farm slot: `func019-service-onboard-schema-r220`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func019-service-onboard-schema-r220 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::service_onboard::tests::action_parser_rejects_future_schema_and_unbounded_correlation_ids -- --exact --nocapture`
- Result: `1 passed; 0 failed; 4748 filtered out`.
