# WL-ARCH-010 cancellation presentation revocation — 2026-08-11

- Scope: Workload cancellation now journals the target with no attachment and
  unavailable readiness before invoking the exact prior lease's external
  revocation or beginning potentially slow backend cleanup. A failed journal
  advance performs neither effect, so restart cannot republish presentation
  authority from a stale durable attachment.
- Regression: `cancellation_revokes_and_journals_presentation_before_slow_backend_cleanup`
  reopens the ledger at the revocation boundary, proves the durable detach is
  already visible, and covers failed persistence without revocation or backend
  cancellation.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=2
  install-helpers/xcp-build.sh cargo test -p mackesd --features async-services
  workers::workload_compute::tests::cancellation_revokes_and_journals_presentation_before_slow_backend_cleanup
  -- --exact --nocapture`.
- Result: **PASS** on `.50`, slot 2 — 1 passed, 0 failed, 4,814 filtered.
- Remaining proof: perform the installed Display1 cancellation/reconnect proof
  on seat 15.
