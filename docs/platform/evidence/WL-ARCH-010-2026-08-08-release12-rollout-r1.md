# WL-ARCH-010 release 12 rollout — 2026-08-08

## Candidate

- Source commit: `6c5f09dbe6beb7ce9a2bdecd9469a17609faa31c`
- Workstation RPM: `magic-mesh-12.1.6-12.x86_64.rpm`
- Workstation SHA-256: `bb94a7981be525b5a029898600c352bf40db5d214dd0da8b861efa9dea62addd`
- Lighthouse RPM: `magic-mesh-lighthouse-12.1.6-5.x86_64.rpm`
- Lighthouse SHA-256: `ad27ff7b69cf94899b55e0bf410281d25b20a1fe819cc3adbbc502bafec470b8`
- Fedora 44 native release build: BigBoy F44 builder `172.20.0.131`, slot `release-6c5f09db-f44`
- Payload gate: workstation 83.6 MiB and lighthouse 12.0 MiB; both passed.

## Physical seats

Each target accepted the exact workstation SHA-256 and passed a separate
`rpm -Uvh --test --replacepkgs --force --nosignature` transaction before the
mandatory warning-and-wait sequence. The warning helper published
`AI-GENERATED-ALERT` and waited five seconds on the five GUI seats.

| target | address | result |
|---|---|---|
| release seat / Basement seat 15 | `172.20.0.15` | release 12; `mackesd`, `nebula`, `syncthing`, and `mde-shell-egui` active; shell restarts `0` |
| Dell | `172.20.146.225` | release 12; `mackesd`, `nebula`, `syncthing`, and `mde-shell-egui` active; shell restarts `0` |
| Eagle | `172.20.146.88` | release 12; `mackesd`, `nebula`, `syncthing`, and `mde-shell-egui` active; shell restarts `0` |
| T480 | `172.20.146.68` | release 12; `mackesd`, `nebula`, `syncthing`, and `mde-shell-egui` active; shell restarts `0` |
| Surface | `172.20.146.79` | release 12; `mackesd`, `nebula`, `syncthing`, and `mde-shell-egui` active; shell restarts `0` |

## Lighthouses

The three current `magic-lighthouse` droplets all accepted the exact
lighthouse SHA-256 and installed release 5. They have no `mde-bus` or visible
seat UI, so the repository warning helper correctly refused there; the helper
was staged temporarily, its five-second delay was honored, and the temporary
copy was removed. The second lighthouse reached `mackesd`, `nebula`, and
`etcd` active. The first and third daemons were still in `Type=notify` startup
readiness when this evidence was recorded; their worker processes were alive,
and no RPM error was logged.

Both slow lighthouses received the narrow runtime correction
`/etc/systemd/system/mackesd.service.d/40-startup-timeout.conf` with
`TimeoutStartSec=0`; the existing 180-second watchdog and stop policy were
unchanged. The first lighthouse was rebooted and its persisted etcd member was
restarted after Nebula returned. The third lighthouse had a full `/run` tmpfs
from stale JSON bus spool files; `mackesd` was stopped, only those disposable
`/run/mde-bus/**/*.json*` files were removed, and the daemon was started again.
All three lighthouses then reported release 5 with `mackesd`, `nebula`, and
`etcd` active, and the three-member etcd health check passed.

## Farm cleanup

Stale rsync-only farm sessions from earlier work were terminated using their
exact process IDs; the active F44 release build was preserved. The local
repository remained clean after the build and rollout work.
