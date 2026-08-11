# WL-UX-013 evidence — missing health projection repair (r221)

- Scope: retained health ingress/projection.
- Change: a missing derived health projection is restored from retained exact
  state without requiring a new Bus message.
- Farm host: `172.20.0.90`.
- Farm slot: `ux013-missing-projection-r221`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-missing-projection-r221 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services workers::health_reconciler::tests::health_ingress_repairs_missing_projection_from_exact_retained_state -- --exact --nocapture`
- Result: `1 passed; 0 failed`.
