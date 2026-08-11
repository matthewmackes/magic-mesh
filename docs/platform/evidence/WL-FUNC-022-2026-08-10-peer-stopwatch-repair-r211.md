# WL-FUNC-022 — peer stopwatch conflict repair (r211)

- Scope: peer stopwatch convergence requires exact payload equality; newer
  conflicting state triggers repair instead of being accepted as converged.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func022-peer-stopwatch-repair-r211-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clock::tests::peer_stopwatch_convergence_repairs_newer_conflicting_payload -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4740 filtered out`.
