# WL-FUNC-018 — admitted App-VM capability projection (r206)

- Scope: App-VM session projection validates the admitted capability policy;
  unsupported host capabilities cannot enter the session roster.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func018-appvm-admitted-capability-r206 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::peer_app_launch::tests::guest_launch_rejects_capabilities_outside_admitted_policy -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4734 filtered out`.
