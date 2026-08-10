# Lighthouse release-11 payload and peer-publication correction — 2026-08-10

## Scope

Live upgrade inspection on lighthouse `104.236.118.177` found two independent
corrected-forward blockers after installing the signed
`magic-mesh-lighthouse-12.1.6-10.x86_64` package:

- `mcnf-mesh-secret-recipient.service` failed because the thin RPM shipped its
  reconciler but omitted the reconciler's package-owned
  `/opt/mcnf/automation/secrets/mcnf-secret.sh` dependency.
- authenticated peer publication was withheld as `machine-id-untrusted`
  because the cloud image had left `/etc/machine-id` group-writable at mode
  `0664`.

The installed release-10 package verified cleanly with `rpm -V`; the missing
helper was therefore a manifest omission, not live filesystem corruption.

## Corrected-forward source

Based on repository revision `e1b960df`:

- the lighthouse variant advances to release 11 and ships the secret helper;
- all daemon-bearing RPM shapes are required to carry the helper, reconciler,
  unit, and timer exactly once;
- the package's existing tmpfiles convergence normalizes `/etc/machine-id` to
  root-owned mode `0444`, preserving the daemon's fail-closed identity reader.

Focused farm command:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=crit007-lighthouse-secret-helper-r116 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  onboard::role_provision::tests::every_daemon_rpm_bootstraps_mesh_secret_recipients \
  -- --exact --nocapture
```

Result: PASS, 1 passed, 0 failed, 4,666 filtered out. The final warm rerun
completed in 0.38 seconds and exercised the exact-once payload assertion and
machine-ID convergence rule.

## Live recovery proof

The live mode was corrected from `0664` to the standard read-only `0444` without
changing machine-ID content. On the next heartbeat, the daemon wrote
`/run/mesh-health/peer-publication.ok` at `2026-08-10 10:27:33 UTC`. The next
watchdog run at `10:28:03 UTC` reported `mesh-health: ok` and completed
successfully. `/run` had recovered to 49% free; the earlier 0% warning stopped
once transient package pressure cleared and was not an arithmetic defect.

## Remaining boundary

Release 11 has not yet been built, signed, or deployed. The installed release
10 lighthouse still lacks the secret helper, so
`mcnf-mesh-secret-recipient.service` remains failed until corrected-forward
package deployment. This checkpoint does not claim three-lighthouse or
six-node convergence.
