# Health toast generation watermark evidence — 2026-08-11

- Scope: ToastHost retains the highest admitted generation for each bounded
  node/condition health authority independently of current and queued toasts.
- Failure behavior: lower/equal replay remains refused after acknowledgement,
  dismissal, or timeout; forward generations still replace. The 256-authority
  lifetime bound fails closed for unseen overflow rather than dropping a known
  watermark and reopening rollback.
- Admission atomicity: when a full all-critical backlog rejects a new toast,
  its provisional generation watermark is rolled back. The same generation can
  be admitted later once capacity exists instead of being falsely classified
  as an already-delivered replay.
- Production path: the shell bridge supplies the governed condition ID and
  snapshot generation from each admitted HealthKironAlert.
- Farm gate: `.170`, slot 1: **1 passed, 0 failed, 293 filtered**.
- Focused backlog-rejection gate: BigBoy, slot 3,
  `toast::tests::rejected_full_backlog_does_not_advance_health_generation_watermark`:
  **1 passed, 0 failed, 294 filtered**.
- Scoped `git diff --check`: passed.
