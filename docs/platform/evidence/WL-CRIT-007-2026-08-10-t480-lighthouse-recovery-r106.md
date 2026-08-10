# WL-CRIT-007 — T480 lighthouse drift and watchdog recovery

Date: 2026-08-10

Seat: T480 (`172.20.146.68`, overlay `10.42.0.8`)

Comparison seat: Dell (`172.20.146.225`, overlay `10.42.0.4`)

## Root cause

T480's Nebula process and `nebula1` interface were active, but no lighthouse
answered. `mesh-health.service` consequently restarted Nebula about every two
minutes. Because `mackesd.target` and its control group require
`nebula.service`, each watchdog restart stopped the grouped daemon; the old
watchdog did not start the target or its six children again.

The installed `magic-mesh-12.1.6-29` and `nebula-1.10.3` payloads, Nebula unit
hashes, drop-in hashes, capability bound, address-family restriction, and
system-call policy matched healthy Dell. `sendto` was allowed, there were no
SELinux or seccomp denials, and repeated ordinary UDP plus exact source-port
4242 probes from T480 succeeded to all configured addresses. A real-process
trace also observed Nebula `sendto(7, ..., 4242)` calls succeeding. The host
firewall, service sandbox, source port, and package bytes were therefore not
the persistent fault.

The authoritative live difference was the lighthouse roster:

- T480's own epoch-0
  `/mnt/mesh-storage/peer:T480/mackesd/nebula-bundle.json` named the retired
  `167.71.247.150`, `104.131.64.207`, and `68.183.55.253` endpoints.
- Healthy Dell used the current `104.236.118.177`, `46.101.219.245`, and
  `64.23.131.57` endpoints for overlays `.1`, `.2`, and `.3`.
- T480's supervisor regenerated `/etc/nebula/config.yaml` from its stale local
  bundle at 23:59:55, proving that a manual config-only correction would recur.
- The old endpoint set produced the journal's intermittent UDP `sendto: operation
  not permitted` errors. Replacing the bundle roster and restarting once
  eliminated both the errors and total overlay loss.

## Corrected-forward live action

Every mutation was preceded by the installed red `AI-GENERATED-ALERT` and its
enforced five-second operator window. No reboot occurred.

Only T480's `lighthouses` array was atomically replaced from Dell's validated
three-member current bundle. T480's CA certificate, peer certificate, mesh ID,
overlay address, trust authority, creation time, and epoch were preserved; the
canonicalized identity-field digest remained
`a6a520cb456e6f2f3553389c5a1322b706dd259d2e867023f7de8eb70968f9c2`.
A mode-0600 rollback copy remains at
`/var/lib/mackesd/recovery-backups/nebula-bundle.pre-crit007-t480-20260810T040639Z.json`.

The supervisor then materialized all three current endpoints into
`/etc/nebula/config.yaml`. Nebula was restarted once at 00:07:12, the target and
six groups were started additively, and the temporary `strace` diagnostic
package was removed. T480's etcd client list was also put in the same
healthy-first order used on Dell and seat 15 (`.3`, `.2`, then degraded `.1`);
the prior file is recoverable at
`/var/lib/mackesd/recovery-backups/etcd-endpoints.pre-crit007-t480-20260810T041320Z`.

## Repository correction and farm gate

`install-helpers/mesh-health-check.sh` now:

- restores `mackesd.target` and only missing grouped children after a Nebula
  restart;
- records an unreachable-overlay restart before acting and suppresses another
  such restart for 600 seconds; and
- clears the cooldown only after a lighthouse answers, while retaining a
  degraded health result during the outage; and
- warns on a timed-out extra coordination probe on a client-only workstation
  without futilely restarting its condition-skipped local `etcd.service`, while
  the lease-backed publication stamp remains the authoritative client result;
  and
- bounds stale-publication recovery to one observation restart per 600 seconds
  instead of restarting that group on every timer tick.

The hostile regression
`install-helpers/test-mesh-health-nebula-recovery.sh` simulates a Nebula restart
that tears down every grouped child. It proves all six are restored, the next
timer pass cannot restart Nebula again, an expired cooldown permits one bounded
retry, and successful lighthouse reachability clears the guard.
The same fixture also proves that a client-only probe timeout with a fresh
publication causes zero local etcd restart attempts, and that persistent stale
publication triggers one observation restart followed by cooldown suppression.

Farm lane `.50`, isolated slot `crit007-t480-r1`, passed exact shell syntax and
the complete hostile regression:

```text
mesh-health nebula recovery hostile regression: passed
```

## Live acceptance

After one full publication cycle and a later timer interval:

- `10.42.0.1`, `.2`, and `.3` each answered two overlay pings;
- Nebula, `mesh-health.timer`, `mackesd.target`, all six grouped services, and
  the DRM shell were active;
- `peer-publication.ok` became fresh at 00:09:58 after observation recovery;
- the timer's 00:09:41 health run completed with `Result=success` and
  `ExecMainStatus=0`;
- Nebula's 00:07:12 start timestamp did not change across the 75-second hold;
  and
- zero `sendto: operation not permitted` records appeared after the corrected
  restart.

The Browser VM has no Nebula process or host certificate. A one-time
`Refusing to handshake with myself` record through the libvirt bridge appeared
immediately after restart and did not repeat; it was not a copied guest
identity.

The final farm-verified watchdog payload was installed byte-for-byte at
`/usr/libexec/mackesd/mesh-health-check` with SHA-256
`9c1024941633d74c8befbb388d7a1aa5b52685a0fa0b6e48f00b78995b6b82e0`.
The release-29 original and the preceding first hotfix remain recoverable at
`/var/lib/mackesd/recovery-backups/mesh-health-check.release29-20260810T041649Z`
and
`/var/lib/mackesd/recovery-backups/mesh-health-check.hotfix1-20260810T042136Z`.

The deliberately hostile live follow-through also exercised the new
publication cooldown. A stale stamp caused one observation restart at 00:21,
and the next two timer passes remained degraded while suppressing another
restart. Publication then committed at 00:23:19. The 00:23:38 timer pass logged
the client-only endpoint-probe warning, accepted the 33-second-old committed
stamp as authoritative, logged `mesh-health: ok`, and exited 0. At that point
all six groups, `mackesd.target`, the shell, timer, and Nebula were active;
Nebula still had its single 00:07:12 start timestamp and `NRestarts=0`. There
were zero later Nebula stops and zero later UDP EPERM records.

## Remaining limits

Lighthouse `.1` answered Nebula traffic but could not commit an etcd proposal;
`.2` and `.3` committed through `etcdctl`, but took about three seconds and
T480's heartbeat client continued to log intermittent lease-backed transaction
failure after the first fresh stamp. Thus the Nebula/target collapse is fixed,
but durable peer publication is not claimed; repairing the slow/degraded quorum
is a separate fleet task. The bundle
format still carries `epoch: 0` and permits divergent same-path content without
a generation/integrity winner; this slice repairs T480 and contains the restart
blast radius but does not redesign that Rust-owned bundle authority. The new
watchdog was installed as an exact farm-verified live hotfix with a rollback
copy, but the next signed package rollout is still required to return T480 to
unmodified RPM payload integrity.
