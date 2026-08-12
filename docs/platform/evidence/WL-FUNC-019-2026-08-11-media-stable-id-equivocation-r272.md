# WL-FUNC-019 media stable-ID equivocation evidence — 2026-08-11

- Scope: the universal resource adapter detects non-identical raw media-source
  observations that claim one projected stable resource identity.
- Boundary: conflict detection occurs before endpoint/path redaction and card
  deduplication, so two hostile rows cannot become one apparently valid card.
  Only the equivocated identity is suppressed; independent valid media/file
  share cards survive and the affected adapter reports `Conflict` without
  leaking the differing locator.
- Regression:
  `media_raw_stable_id_equivocation_is_visible_before_redacted_card_deduplication`.
- Result: **PASS**. Farm `.90`, slot `func019-media-equivocation`, ran the exact
  regression after a clean full test-profile compilation: 1 passed, 0 failed,
  4,923 filtered. Farm `.90`, slot `func019-clippy`, ran
  `cargo clippy -p mackesd --features async-services --lib` to completion with
  warnings only (3,442 warnings).
- Remaining proof: retain the epic's live signed publisher/recovery acceptance.
