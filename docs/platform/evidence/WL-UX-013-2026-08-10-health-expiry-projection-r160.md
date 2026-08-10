# WL-UX-013 health expiry projection — r160

- Revision: `e2ad474c`
- Scope: expired checkpoint state is rejected against live time; stale invalid projections are removed when no last-good state exists, while symlinks remain untouched.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux013-health-expiry-r160 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::health_reconciler::tests::health_ingress_drops_expired_checkpoint_state_and_stale_projection -- --nocapture`
- Result: `1 passed; 0 failed; 4700 filtered out` on BigBoy.

