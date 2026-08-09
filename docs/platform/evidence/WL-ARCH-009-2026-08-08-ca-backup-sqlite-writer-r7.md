# WL-ARCH-009 CA-backup SQLite writer cutover — 2026-08-08

The five direct-write syntax sites in `ca/backup.rs` now use typed writer
operations. CA restore is one bounded transaction, refuses conflicting rows,
treats an identical restart replay as a no-op, and preserves the audit chain.
Operational backup/restore tests retain their typed fixture setup.

## Verification

- `.170`, slot `arch009-ca-backup-r7`: 6/6 focused tests passed.
- `.90`, slot `arch009-ca-backup-lint-r7`: SQLite authority self-test and
  actual lint passed with 14 residual reviewed sites, down from 19.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Fourteen allowlisted direct-write sites and live six-group writer failure and
restart proof remain, so ARCH-009 stays `Remaining`.
