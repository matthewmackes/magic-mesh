# WL-FUNC-019 evidence — Android readiness action projection (r220)

- Scope: Remote Sessions resource adapters.
- Change: Android catalog Start actions remain visible as evidence but are
  marked `Unavailable/NotObserved` until live guest readiness is observed,
  preventing an executable-looking action that the router must reject.
- Farm host: `172.20.0.90`.
- Farm slot: `func019-android-readiness-action-r220`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func019-android-readiness-action-r220 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::service_aggregator::resource_adapters::tests::android_catalog_cards_bind_exact_workload_and_gate_unobserved_start_action -- --exact --nocapture`
- Result: `1 passed; 0 failed`.
