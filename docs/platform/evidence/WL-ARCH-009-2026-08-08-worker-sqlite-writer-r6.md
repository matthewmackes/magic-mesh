# WL-ARCH-009 worker SQLite writer cutover — 2026-08-08

The reconciler's seven remaining direct-write syntax sites now cross typed
writer operations. Reconcile events are admitted as a bounded batch and commit
in one transaction while preserving the audit hash chain. An exact restart
replay returns no new rows; a partial durable replay fails closed. Test fixture
setup also uses a typed test-only writer operation, so `worker.rs` contains no
direct SQLite mutation syntax.

## Verification

- `.170`, slot `arch009-worker-s6`: seven focused worker tests passed.
- Writer batch atomicity/replay test: 1/1 passed.
- `.90`: SQLite authority self-test and actual lint passed with 19 residual
  reviewed sites, down from 26.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Nineteen allowlisted direct-write sites and live six-group writer failure and
restart proof remain, so ARCH-009 stays `Remaining`.
