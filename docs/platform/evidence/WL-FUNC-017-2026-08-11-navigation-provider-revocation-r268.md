# WL-FUNC-017 navigation provider revocation evidence — 2026-08-11

- Scope: navigation route admission now revalidates its root-governed provider
  authority after route-provider I/O and before accepting the result.
- Boundary: provider ID, loopback endpoint, timeout, and Ed25519 verification
  key must still exactly match the authority used to issue and authenticate the
  request. Replacement or revocation during calculation produces typed
  `ProviderNotConfigured` state instead of publishing the old provider's route.
- Hostile regression:
  `provider_authority_replacement_during_calculation_revokes_result` replaces
  the authority with a different signing key while the provider request is in
  flight and proves the otherwise valid old-key response is refused.
- Intended farm command: `cargo test -p mackesd --features async-services workers::navigation::tests::provider_authority_replacement_during_calculation_revokes_result -- --exact --nocapture`.
- Result: **INFRASTRUCTURE FAILURE, NOT A TEST FAILURE**. A governed BigBoy slot
  admitted the command with 15.6 GiB free, but concurrent cold links expanded
  `/home`; mold then failed to write the test binary with `Disk full?` before the
  regression executed. The failed slot's disposable target cache was removed,
  restoring 17.0 GiB without touching source or durable artifacts.
- Remaining proof: rerun the exact gate on a warm governed lane and retain the
  epic's offline/live route and MG90 acceptance.
