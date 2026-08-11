# WL-UX-013 heartbeat byte bound — 2026-08-11

- Scope: peer health heartbeat fallback.
- Change: heartbeat fallback now uses the existing regular-file byte bound before JSON parsing; oversized, symlinked, and non-regular heartbeats fail closed as unreachable.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux013-heartbeat-bound-r224 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services workers::health_reconciler::tests::oversized_heartbeat_is_rejected_before_json_parse -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
