# WL-ARCH-009 grouped-daemon crash isolation — 2026-08-08

One grouped daemon failure no longer deactivates `mackesd.target` or restarts
otherwise-healthy groups. The RPM upgrade path also restarts an already-active
grouped target after replacing its binary and unit relationships.

## Release 21 failure and root cause

An authorized live `SIGKILL` of `mackesd-integrations.service` on Dell Release
21 changed all six process PIDs. The pre-crash PIDs were control `25802`,
observation `26032`, actions `26025`, data `26030`, compute `26181`, and
integrations `26037`. The journal recorded integrations failing, followed by
`mackesd.target` stopping and `PartOf=mackesd.target` propagating that stop to
the five healthy groups.

The target used `Requires=` for every grouped service, and the five non-control
services used `Requires=mackesd-control.service`. Those requirement edges made
an individual process failure a target-wide failure. They now use `Wants=` with
the existing `After=` ordering; `nebula.service` remains a required substrate.
The source validator rejects either crash-cascade relationship and includes a
hostile self-test fixture.

Release 22 package inspection then found that an active grouped target was not
restarted during upgrade. Release 23 records whether `mackesd.target` was
active and `try-restart`s it after package setup, ensuring the replacement
binary and dependency graph are actually live. The package activation gate now
enforces that ordering.

## Farm and artifact verification

- Machine 9 (`172.20.0.50`), slot `arch009-upgrade-contract-r2`: the RPM seat
  activation contract and extracted shell syntax passed.
- Machine 193 (`172.20.0.90`), slot `arch009-boundary-r2`: process-boundary
  self-test and source validation passed.
- Earlier package gates in slots `arch009-crash-isolation-rpm-r1` and
  `arch009-crash-isolation-contract-r1` passed grouped base/server/lighthouse
  payload and upgrade-lifecycle checks.
- BigBoy (`172.20.0.130`), slot `arch009-release22-f44-r1`, produced Fedora 44
  `magic-mesh-12.1.6-23.x86_64.rpm` at 85.6 MiB. Its SHA-256 is
  `9ffcba3861aaad07098dfea64215d6a407a0e2212261b858f77846f2c6c29148`.
  The Fedora base was tag-pinned rather than digest-pinned, so this is an
  engineering live-proof artifact, not a reproducible production release.
- The RPM payload contains all six grouped units and the target with the new
  `Wants=` edges. Its scriptlet contains active-target capture and the ordered
  grouped-target restart. The seat transaction test passed before installation.

## Live corrected-forward proof

Dell was offline (`No route to host`) for corrected-package deployment, so the
same physical Fedora 44 Workstation proof ran on seat 15,
`Basement-Test-Workstation` (`172.20.0.15`). It upgraded from Release 21 to
Release 23. The package-owned upgrade restart changed all six old PIDs, all six
services returned active, `mackesd.target` remained active, and `rpm -V
magic-mesh` produced no output.

An integrations crash produced this exact isolation matrix:

| Group | Before PID/restarts | After PID/restarts |
| --- | --- | --- |
| control | `288251 / 0` | `288251 / 0` |
| observation | `288449 / 0` | `288449 / 0` |
| actions | `288440 / 0` | `288440 / 0` |
| data | `288442 / 0` | `288442 / 0` |
| compute | `288445 / 0` | `288445 / 0` |
| integrations | `288447 / 0` | `291790 / 1` |

A subsequent control crash recovered in six seconds while retaining every
worker process:

| Group | Before PID/restarts | After PID/restarts |
| --- | --- | --- |
| control | `292633 / 0` | `296764 / 1` |
| observation | `288449 / 0` | `288449 / 0` |
| actions | `288440 / 0` | `288440 / 0` |
| data | `288442 / 0` | `288442 / 0` |
| compute | `288445 / 0` | `288445 / 0` |
| integrations | `291790 / 1` | `291790 / 1` |

After both crashes, the target and every group were active, an explicit
`mesh-health.service` pass succeeded, no grouped service was failed, and RPM
verification remained clean.

## Remaining acceptance gap

The S4 group-crash isolation and active-upgrade portions are proven on one live
physical seat. Complete ownership/provider assignment, optional-worker
quiescence, declared cgroup/resource refusal across all six groups, responsive
Workers captures, and fleet convergence remain; ARCH-009 stays `Remaining`.
