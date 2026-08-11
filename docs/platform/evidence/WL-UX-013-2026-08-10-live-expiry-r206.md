# WL-UX-013 — live health projection expiry (r206)

- Scope: expired retained health projections are evicted during live ingress;
  stale same-process state cannot survive until daemon restart.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-live-expiry-r206 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::health_reconciler::tests::live_ingress_expires_retained_state_without_waiting_for_restart -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4735 filtered out`.
