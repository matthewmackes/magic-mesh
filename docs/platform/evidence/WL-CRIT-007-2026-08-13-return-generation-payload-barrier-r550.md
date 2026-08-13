# WL-CRIT-007 return generation/payload barrier — 2026-08-13 r550

## Production change

`install-helpers/run-corrected-forward-recovery-probe.sh` now performs a second
read-only target admission after SSH returns and before it starts
`mcnf-peer-recovery.service`. The returning node must have a new boot ID while
retaining the exact preflight target, role, overlay, session user, package NEVRA,
and non-null SHA-256 RPM payload digest. A stale boot generation or substituted
identity/package/payload refuses before peer-recovery mutation. The final
post-recovery snapshot is checked against the same preflight authority again.

This closes a pre-release ordering gap: network return alone no longer permits a
different installed payload or target generation to exercise the authenticated
recovery path.

## Hostile self-test

The production script's `--self-test` covers one admitted transition and rejects:

1. an unchanged boot generation;
2. a substituted target;
3. a substituted package NEVRA; and
4. a substituted package payload digest.

## Gates

Farm node `172.20.0.90`, slot 1 (`magic-mesh-farm-1`):

- `bash install-helpers/run-corrected-forward-recovery-probe.sh --self-test` —
  passed 5/5.
- `bash -n install-helpers/run-corrected-forward-recovery-probe.sh` — passed.

The farm sync excludes `.git`, so the chained farm `git diff --check` was
inapplicable after both gates above passed. The scoped repository
`git diff --check` passed locally. The first attempted farm command used an
unsupported helper mode and the next used an obsolete workspace path; neither
executed the production gate.

No live reboot, sleep, release acceptance, or multi-node proof was performed.
Those remain deferred until after the first full release.

## Residual acceptance

- Cut and install the first complete signed release.
- Run one-node boot, suspend/resume, network-return, and corrected-forward
  recovery against its exact signed payloads.
- Record the deferred non-blocking physical recovery evidence and any required
  lighthouse coordination evidence.
