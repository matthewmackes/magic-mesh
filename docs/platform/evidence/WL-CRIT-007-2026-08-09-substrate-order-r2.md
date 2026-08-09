# WL-CRIT-007 configured substrate recovery order — 2026-08-09

## Outcome

Configured etcd and Syncthing are now recovery dependencies, not best-effort
services. If either cannot start and reach active state, peer recovery publishes
an exact failure and stops before XDG bind repair or grouped mackesd restart.
Previously it accumulated a degraded flag, continued downstream mutation, and
only failed after workers had already run without their configured coordination
or synchronization substrate.

The hostile etcd-start fixture proves the mutation ledger contains only the
etcd attempt. Existing offline refusal, bounded Nebula retry, boot-race
preservation, grouped-child readiness, already-healthy idempotency, trigger
coalescing, and resume/online trigger fixtures remain active.

## Farm verification

- Machine 193 (`172.20.0.90`), slot `arch010-r12-lints`: shell syntax and
  `sudo -n install-helpers/test-mesh-peer-recovery.sh` passed every fixture,
  including the new configured-etcd failure boundary.

## Source hashes

- `62b719b2dfef792164fad0fa02d969eb6526eb131bbf480a3d90308b1eb8e127`
  — `install-helpers/mesh-peer-recovery.sh`
- `11876ca511a6fb0281d89f5ce37eb1e7a856566e08ac4671278ff39c4a10f074`
  — `install-helpers/test-mesh-peer-recovery.sh`

## Remaining boundary

This proves disposable fault ordering only. Physical suspend/resume and the
remaining Eagle, T480, Surface, and lighthouse corrected-forward matrix keep
WL-CRIT-007 `Remaining`.
