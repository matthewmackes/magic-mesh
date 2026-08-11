# WL-FUNC-011 call-media proof attribution — 2026-08-11

- Scope: Calls media verification now checks that each candidate adapter is
  compatible with the declared call kind and that its requirement set is exact,
  ordered, and non-vacuous before invoking a provider. Misattributed, altered,
  reordered, or empty requirements produce `MediaNotProven` with no evidence.
- Regression: `verifier_refuses_misattributed_or_vacuous_provider_evidence`
  registers panic-on-call providers and proves invalid readiness never consumes
  provider evidence.
- Intended focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd
  --features async-services
  workers::collab_media::tests::verifier_refuses_misattributed_or_vacuous_provider_evidence
  -- --exact --nocapture`.
- Result: **NOT RUN**. Every farm host was below the governed 8 GiB `/home`
  reserve, so no host/slot was selected or synced and no safety bypass was used.
- Remaining proof: run the exact regression from a warmed/safe slot. No
  production call-media provider exists yet; future providers must supply fresh,
  session-bound sampling evidence and live call proof.
