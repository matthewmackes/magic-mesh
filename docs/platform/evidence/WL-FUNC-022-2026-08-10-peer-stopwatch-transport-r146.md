# WL-FUNC-022 — peer stopwatch transport and clock-domain authority (r146)

Date: 2026-08-10

Source revision: working source after c678bc07 plus the peer transport slice.

## Result

Signed approved peer stopwatch updates now reach selected mirror targets and
remain read-only there. Admission requires the command origin to match the
stopwatch origin, requires the local node to be a mirror target, preserves the
origin revision, rejects stale/equal-conflicting revisions, and refuses
unapproved, blocked, untargeted, cross-origin, and locally forged variants.

Origin nodes continue to stamp their own stopwatch revisions. Peer monotonic
timestamps remain opaque across machines; local command issuance remains
responsible for validating its own monotonic clock domain.

## Focused farm proof

BigBoy build VM .130, slot func022-stopwatch-peer-r143:

~~~text
cargo test -p mackesd --lib peer_stopwatch_ -- --nocapture
~~~

The final synced source passed both exact tests: 2 passed, 0 failed, 4,683
filtered.

Focused rustfmt and git diff --check passed locally. No physical seat was
used.

## Remaining boundary

Stale/frozen mirror presentation,
complete Clock UI/package integration, broader multi-process peer traces, and
installed-seat proof remain.

*** Update File: /root/magic-mesh/docs/platform/WORKLIST.md
@@
- **Stopwatch origin-authority checkpoint (2026-08-10):** mirrored controls, forged origins, and identity transfer now fail closed; BigBoy/.50 passed 3/3:
  docs/platform/evidence/WL-FUNC-022-2026-08-10-stopwatch-origin-authority-r143.md.
- **Peer stopwatch transport checkpoint (2026-08-10):** approved targeted mirrors preserve origin/revision and hostile variants fail closed; BigBoy exact rerun pending:
  docs/platform/evidence/WL-FUNC-022-2026-08-10-peer-stopwatch-transport-r146.md.
