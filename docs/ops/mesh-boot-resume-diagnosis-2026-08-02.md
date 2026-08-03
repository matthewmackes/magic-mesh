# Mesh boot / sleep-resume diagnosis — 2026-08-02

## Live evidence

The affected live workstation (`Basement-Test-Workstation`, `.15`) had
`nebula.service` and `mackesd.service` running, but all configured etcd
endpoints were unhealthy. `etcd.service` was skipped because this workstation
is a client-only node and has no `/etc/etcd/etcd.env`. Consequently, peer
heartbeats, the etcd watch, and leader election repeatedly failed; the peer
directory could not repopulate after a boot or wake.

The workstation also has a stale identity/configuration collision: its active
certificate is named `peer:Basement-Test-Workstation` but claims
`10.42.0.1/17`, the founding lighthouse address, and its generated config says
`am_lighthouse: true`. The live mesh inventory reserves `10.42.0.1` for the
founding lighthouse. Do not repair this by editing the certificate in place;
re-enroll the workstation with a newly allocated peer address after the
coordination quorum is healthy.

Nebula's journal also recorded invalid-certificate handshakes for a lighthouse
address, consistent with the duplicate overlay identity. This is sufficient to
explain peers being unreachable or rejected after boot/wake.

## Code correction made

`install-helpers/mesh-health-check.sh` previously exited unless the legacy
flat `/etc/nebula/host.crt` existed. Current enrollment stores the active
certificate at `/etc/nebula/identity/current/host.crt`, so the watchdog was a
no-op on migrated laptops. The check now accepts either layout.

`packaging/systemd/mackesd.service` now orders `mackesd` after
`nebula.service` while keeping Nebula a soft dependency. This removes the boot
race against creation of `nebula1` without preventing degraded startup.

## Required operator recovery

1. Restore a healthy etcd quorum on the three lighthouse members and verify
   `etcdctl endpoint health` from the workstation.
2. Re-enroll `.15` as an ordinary peer so its certificate receives a unique
   `10.42.0.x` address and renders `am_lighthouse: false`.
3. Reboot and perform a suspend/resume proof. Check `nebula1`, peer handshakes,
   etcd health, Syncthing connections, and `verify-boot-recovery.sh`.

Do not record SSH, HTTP, CA, or MG90 credentials here.

## Recovery completed for bug testing

The live recovery was completed on 2026-08-02:

- Lighthouse `.1` `104.236.118.177`, `.2` `46.101.219.245`, and `.3`
  `64.23.131.57` now have cross-lighthouse Nebula maps and a healthy
  three-member etcd quorum.
- `Basement-Test-Workstation` was re-enrolled as a workstation at `10.42.0.5`
  and its tunnel was fully restarted, rather than hot-reloaded across the
  address change.
- The seat reaches all three current lighthouse overlay addresses and reports
  them through the coordination plane. Its mackesd, Nebula, Syncthing, and
  mesh-health timer are active.
- The old seat relay-authority files and pre-recovery Nebula configuration were
  retained with `.pre-reenroll-20260802` / `.pre-recovery-20260802` suffixes;
  no credentials were copied into the repository.

The seat still reports a degraded disk-headroom alarm; this is independent of
the overlay return path and should remain visible during testing.

Dell (`DELL-LAPTOP`, LAN `172.20.146.225`, overlay `10.42.0.4`) was online but
still had retired lighthouse underlay addresses. Its current maps were
restored and its Nebula restart now reaches all three active lighthouses; the
peer directory reports Dell online and healthy.
