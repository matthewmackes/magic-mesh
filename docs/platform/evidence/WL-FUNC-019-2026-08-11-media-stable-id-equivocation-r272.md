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
- Result: **NOT RUN**. BigBoy slot 3 had 17 GiB free, but the shared dirty tree's
  concurrently edited Android lifecycle file failed compilation before this
  exact test executed. Targeted `git diff --check` passed; no result is inferred.
- Remaining proof: rerun the exact adapter regression once the Android compile
  dependency stabilizes, then retain the epic's live signed publisher/recovery
  acceptance.
