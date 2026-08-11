# WL-FUNC-018 first-launch cold boot — 2026-08-11

- Scope: the production `action/cloud/app-provision` path interprets Front
  Door's idempotent resume intent as resume-when-evidence-exists or cold boot
  when no guest evidence exists. A first boot now publishes an identity-bound,
  signed Workload `StartAndAttach` request with the admitted App-VM image,
  resources, node, Display1 attachment, stable operation ID, and bounded
  deadline. Runtime evidence comes from the dynamically resolved production
  Bus; stale, terminal, malformed, and cross-VM evidence remain fail-closed.
- Farm: initial cold-boot gate on BigBoy `172.20.0.130`; integrated handoff and
  replay gates on `172.20.0.90`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::cloud::verbs::app::tests::front_door_resume_intent_cold_starts_without_prior_guest_evidence -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,794 filtered out.
- Integrated focused gates:
  - `workers::cloud::verbs::app::tests::production_app_provision_publishes_identity_bound_workload_start_and_attach`: PASS, 1 passed, 0 failed, 4,800 filtered out.
  - `workers::workload_compute::tests::rotated_token_replay_is_idempotent_but_semantic_change_conflicts`: PASS, 1 passed, 0 failed, 4,801 filtered out.
- Authenticated VDI handoff gates on `.90`:
  - `front_door::tests::app_provision_authenticates_initiating_client_peer_fail_closed`:
    PASS, 1 passed, 0 failed, 1,548 filtered out;
  - `workers::cloud::verbs::app::tests::production_app_provision_publishes_identity_bound_workload_and_open_app`:
    PASS, 1 passed, 0 failed, 4,804 filtered out.
- VDI behavior: Front Door places the enrolled initiating local peer inside the
  signed provision digest and refuses missing, malformed, or fallback
  `localhost` identity before authorization. Cloud validates that identity,
  publishes typed `OpenApp` only after `StartAndAttach`, and binds the exact
  session, workload, serving/client peers, catalog, profile, and capabilities.
  A replay after failed publication recovers with a fresh capability while the
  session broker folds the unchanged semantic request idempotently.
- Replay behavior: a rotated one-use capability is delivery metadata for an
  already-authorized operation; every semantic lifecycle field remains exact,
  conflicting reuse fails, and no backend effect repeats.
- Remaining boundary: guest readiness, stop/cleanup, image supply, and live
  three-seat security/presentation proof remain.
