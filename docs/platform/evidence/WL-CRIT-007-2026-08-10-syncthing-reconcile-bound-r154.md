# WL-CRIT-007 — bounded Syncthing reconciliation (r154)

Date: 2026-08-10

The Syncthing peer reconciler now shares one lock across timer and manual
invocations and bounds systemctl, Syncthing CLI, and etcd reads. A slow or
stalled registry cannot leave overlapping oneshots consuming CPU on a seat.

## Farm proof

Build VM `.50` (`172.20.0.50`), slot `crit007-syncthing-bound-r154`:

```text
bash install-helpers/test-syncthing-device-scope.sh
PASS: Syncthing managed-folder device-scope self-test
```

The test covers authoritative/offline reconciliation, no restart/deletion,
and stale own-publication health behavior; live seat steady-state sampling
remains open.
