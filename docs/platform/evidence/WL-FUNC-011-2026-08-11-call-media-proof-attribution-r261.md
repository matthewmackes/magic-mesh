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
- Result: **PASS**. Farm `.90`, slot `func011-call-media`, ran the exact
  regression after full test-profile compilation: 1 passed, 0 failed, 4,923
  filtered. Farm `.90`, slot `func011-clippy2`, ran
  `cargo clippy -p mackesd --features async-services --lib` to completion with
  warnings only (3,442 warnings).
- Remaining proof: no production call-media provider exists yet; future
  providers must supply fresh, session-bound sampling evidence and live call
  proof.
