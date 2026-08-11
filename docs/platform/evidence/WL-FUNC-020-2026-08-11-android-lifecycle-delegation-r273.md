# WL-FUNC-020 Android lifecycle delegation evidence — 2026-08-11

- Scope: Android Start and Stop delegate exclusively through signed typed
  `WorkloadOperationRequest` rows; the Cloud lane performs no direct backend
  effect.
- Provenance boundary: every operation reloads the admitted signed Android
  catalog and requires the desired image ID/digest plus package manifest to
  match exactly. Missing, legacy, stale, cross-workload, or mismatched rows are
  quarantined before Workloads publication. Exact replay preserves semantic
  request/workload/node/generation/resources and rotates only delivery nonce.
- Cancel remains a typed refusal because Android lifecycle v1 lacks the concrete
  prior `target_request_id` required for safe Workloads cancellation.
- BigBoy (`172.20.0.130`) slot 2 exact gates:
  - `typed_stop_binds_workload_generation_and_exact_replay_while_cancel_stays_refused`: PASS — 1 passed, 0 failed, 4,821 filtered.
  - `restart_quarantines_legacy_provenance_and_preserves_exact_current_start`: PASS — 1 passed, 0 failed, 4,821 filtered.
- Targeted `git diff --check` passed.
- Remaining work: extend the Android input contract before Cancel delegation,
  then obtain live Cuttlefish Start/Stop/VDI proof.
