# WL-UX-011 — unexplained unavailable control refusal (r207)

- Scope: Enable/Disable actions are refused when a provider reports an
  unexplained unavailable state, before authorization or mutation.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux011-unavailable-control-r207 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::device_control::tests::unavailable_provider_state_cannot_reach_the_mutation_seam -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4737 filtered out`.
