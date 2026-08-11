# WL-FUNC-019 — service action requires fresh probe confirmation (r217)

- Scope: advertised-only and stale service rows remain visible for diagnosis but do not expose an actionable verb; only a fresh probe-confirmed row receives the mapped action.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func019-aggregate-action-clean-r1 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::service_aggregator::aggregate::tests -- --nocapture`.
- Result: `.90` passed: `6 passed; 0 failed; 0 ignored; 0 measured; 4739 filtered out`.
