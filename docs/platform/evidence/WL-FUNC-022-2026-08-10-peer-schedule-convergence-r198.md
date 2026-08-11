# WL-FUNC-022 — exact peer schedule convergence (r198)

- Scope: peer Clock schedule convergence requires exact payload equality, not
  revision-only identity, preventing conflicting same-revision schedules from
  being accepted.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func022-exact-schedule-convergence-r198 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clock::tests::peer_schedule_convergence_rejects_revision_only_matches -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4731 filtered out`.
