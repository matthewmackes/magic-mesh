# WL-ARCH-009 CA-revoke SQLite writer cutover — 2026-08-08

All four direct-write sites in `ca/revoke.rs` now use typed writer operations.
Revocation is transactional, refuses certificate resurrection, preserves the
audit/hash history, rolls back failures, and replays idempotently after restart.

## Verification

- `.170`: CA revoke tests passed 5/5 and process-isolated writer proof passed
  1/1.
- `.90`: SQLite authority self-test and actual lint passed with five residual
  reviewed sites, down from nine.
- Scoped formatting and `git diff --check` passed.

## Remaining acceptance gap

Five allowlisted direct-write sites and live six-group writer failure/restart
proof remain, so ARCH-009 stays `Remaining`.
