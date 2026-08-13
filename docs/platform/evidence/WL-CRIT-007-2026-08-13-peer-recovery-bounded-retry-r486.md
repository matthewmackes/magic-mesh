# WL-CRIT-007 peer-recovery bounded retry — 2026-08-13

## Scope

Boot, resume, and network-return recovery is edge-triggered. A transient
Nebula, etcd, Syncthing, grouped-daemon, or desktop recovery failure therefore
could leave `mcnf-peer-recovery.service` failed indefinitely when no later
network event arrived. The service now uses `Restart=on-failure` with a
five-second delay. Its existing `StartLimitIntervalSec=120` and
`StartLimitBurst=6` keep retries bounded. Intentional offline/no-mutation paths
remain successful helper exits and do not restart while a node is disconnected.

Changed implementation:

- `packaging/systemd/mcnf-peer-recovery.service`

## Farm evidence

- Host `.50` (`172.20.0.50`), slot
  `crit007-peer-recovery-retry-unit-r486`:
  `systemd-analyze verify --root="$probe_root" mcnf-peer-recovery.service`
  against a temporary package-shaped root, followed by exact assertions for
  `Restart=on-failure`, `RestartSec=5s`, `StartLimitIntervalSec=120`, and
  `StartLimitBurst=6`. Result: **PASS** (exit 0). The reduced synthetic root
  emitted the expected `sysinit.target not found` diagnostic; the unit and
  packaged executable were both present and accepted.
- Host `.90` (`172.20.0.90`), slot
  `crit007-peer-recovery-retry-payload-r486`:
  `install-helpers/verify-rpm-payload.sh --self-test`. Result: **PASS**; every
  payload/manifest/Requires assertion passed.

An additional full peer-recovery fixture run on `.170` was not used as passing
evidence: concurrent, uncommitted changes in the separately owned helper/test
scope introduced an expected-mutation mismatch after earlier fixtures passed.
Those files were preserved and are not part of this slice.

## Remaining acceptance

This proves bounded corrected-forward retry configuration, not physical fleet
recovery. WL-CRIT-007 still requires direct boot, suspend/resume, network-return,
reboot, and upgrade evidence on the selected physical seats and required
lighthouses, including one authenticated peer/session, synchronized substrate,
no stale duplicate identity/process/session, and actionable failure state.
