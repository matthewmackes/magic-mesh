# WL-ARCH-009 revision/event SQLite writer cutover — 2026-08-08

Revision rollback creation and hash-chained event insertion now cross strict
typed operations into the process-isolated writer. The writer validates
identity, timestamps, JSON depth/size/fields, performs each mutation
transactionally, preserves event hash-chain integrity, and treats an exact
restart replay idempotently. Both caller files now contain zero direct-write
syntax; the checked baseline fell from 35 to 31 residual sites.

## Verification

BigBoy `.130`, slot `arch009-revisions-events-writer-r1`:

- Writer tests: 7/7 passed.
- Event tests: 6/6 passed.
- Restart replay, transaction rollback, hostile payload refusal, and intact
  event hash chains were exercised.
- SQLite authority self-test and actual lint passed with 31 residual sites.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Thirty-one allowlisted direct-write sites and the live six-group writer
failure/restart proof remain, so ARCH-009 stays `Remaining`.
