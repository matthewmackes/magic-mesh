# WL-FUNC-018 App catalog publisher provenance — r498

Date: 2026-08-13

## Implemented boundary

The reachable universal service catalog previously derived App-card
`OperatorDeclared` provenance from `ServicesState.host` after applying the
lossy `safe_id` transformation. Distinct inputs such as `seat/15`, `seat 15`,
and `seat-15` could therefore alias the same fresh App provider identity.

`service_catalog` now admits the publishing node identity before constructing
any App/profile card. The source must already be non-empty and canonical;
empty, case-folded, separator-substituted, whitespace-bearing, and
control-bearing publishers fail closed. The exact admitted publisher is used
for the catalog publisher, revision, and App provenance source ID.

## Farm evidence

- BigBoy `.130`, slot `func018-app-publisher-test-r498`: the fully qualified
  focused regression
  `workers::service_catalog::tests::app_catalog_rejects_lossy_publisher_provenance_before_projection`
  passed 1/1 with 4,947 filtered tests. It rejects five hostile/ambiguous
  publisher forms and preserves canonical `seat-15` App provenance.
- BigBoy `.130`, slot `func018-app-publisher-clippy-r498`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.170`, slot `func018-app-publisher-fmt-r498`: exact-file
  `rustfmt --edition 2021 --check` reported pre-existing formatting drift in
  five untouched regions (`registered_card`, service freshness, RDP
  provenance, a prior service test, and a prior configuration fixture). It
  reported no difference in either r498 hunk. Those unrelated lines were
  deliberately preserved; the r498 changed hunks are formatter-clean.

An initial short-name `--exact` invocation selected zero tests and is rejected
as evidence. The corrected fully qualified invocation above is the claimed
test result.

## Remaining acceptance

This closes one catalog publisher-provenance gap only. WL-FUNC-018 still needs
the current App VM image/profile artifact, launch/readiness and lifecycle
completion, packaging, and the deferred post-release installed App VM/VDI
acceptance matrix.
