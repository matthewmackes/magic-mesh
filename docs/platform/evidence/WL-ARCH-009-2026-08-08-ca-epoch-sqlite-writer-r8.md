# WL-ARCH-009 CA-epoch SQLite writer cutover — 2026-08-08

All five direct-write syntax sites in `ca/epoch.rs` now use bounded typed writer
operations. Epoch transitions remain atomic, reject conflicts, replay safely
after restart, and preserve the audit hash chain.

## Verification

- `.170`: CA epoch tests passed 10/10 and the process-owner writer test passed
  1/1.
- `.90`: SQLite authority self-test and actual lint passed with nine residual
  reviewed sites, down from 14.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Nine allowlisted direct-write sites and live six-group writer failure/restart
proof remain, so ARCH-009 stays `Remaining`.
