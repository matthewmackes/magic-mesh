# WL-FUNC-020 canonical guest-runtime artifact staging — 2026-08-13

## Scope

This evidence records the first successful canonical staging self-test for the
production Cuttlefish readiness-relay and VDI-agent artifacts after the two
preceding runs exposed and corrected staging defects. No other gate was rerun.

## Immutable source and farm lane

- Source revision: `c572d087ab8a269d22d544f6917743272ad3c612`
- Build host: `172.20.0.130` (BigBoy)
- Isolated slot: `func020-staging-canonical-r524`
- Command: `bash packaging/android/test-stage-guest-runtime-artifacts.sh`

The isolated farm repository was reset to the exact source revision above and
that identity was checked before invoking the command.

## Captured result

```text
Finished `release` profile [optimized] target(s) in 1m 33s
Cuttlefish guest release artifacts staged: /tmp/tmp.r3wID4a2lt/good
Cuttlefish guest artifact stage verified: /tmp/tmp.r3wID4a2lt/good
Cuttlefish guest artifact staging hostile self-test passed
```

Exit status: `0`.

The self-test exercised canonical locked release construction and staging, then
confirmed exact executable source identity and manifest binding while rejecting
stale-revision, wrong-architecture, and mutable-artifact hostile cases.

## Disposition

The previously unproven FUNC-020 guest-runtime staging coding verification is
green at `c572d087`. First-release integration and post-release acceptance/live
proof remain outside this focused coding gate.
