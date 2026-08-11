# WL-FUNC-021 workspace request binding — 2026-08-11

- Scope: the installed `mde-musicd serve` workspace mutation ledger now binds
  every request ID to the normalized action SHA-256. Exact replay returns its
  durable result; reuse with a different playback/catalog/cast/bookmark action
  fails closed across restart. Legacy v1 rows remain readable but digest-less
  replay is refused, and the next write upgrades the ledger to v2.
- Farm: `172.20.0.50`, slot `2`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mde-musicd bus_responder::tests::workspace_ledger_rejects_conflicting_request_id_across_restart -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 246 filtered out.
