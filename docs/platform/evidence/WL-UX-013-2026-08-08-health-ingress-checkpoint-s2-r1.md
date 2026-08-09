# WL-UX-013 health ingress checkpoint — 2026-08-08

The health reconciler now consumes only the exact retained health topic/file
for each approved publisher. It validates every bounded envelope and maintains
a per-publisher revision ledger, rejecting replay, rollback, malformed, stale,
or publisher-mismatched observations without replacing the last accepted
state.

The ledger is persisted as a per-observer checkpoint using a bounded 16 MiB
read, same-directory temporary file, file sync, atomic rename, and parent
directory sync. Oversized, corrupt, or symlinked checkpoints fail closed, so a
daemon restart cannot silently erase replay protection or accept an ambiguous
health history.

## Verification

- Farm `.50`, slot `ux013-health-checkpoint-s3-r2`: focused
  `health_reconciler` suite passed 24/24.
- The current integrated tree repeated the same 24/24 result on `.170`, slot
  `integrated-health-checkpoint-s6-r1`.
- Fixtures cover exact ingress, independent publisher advancement, replay and
  rollback refusal, restart restoration, corrupt and oversized checkpoint
  refusal, symlink refusal, and atomic checkpoint replacement.
- Scoped formatting and whitespace checks passed.

## Remaining acceptance gap

Provisioned multi-node health publication, live publisher loss/rejoin, and GUI
drill-down proof remain, so UX-013 stays `Remaining`.
