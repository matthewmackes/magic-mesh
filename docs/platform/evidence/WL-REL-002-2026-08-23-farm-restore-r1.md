# Farm restore — canonical F42 roster + slot GC r1

Date: 2026-08-23  
Classification: farm recovery; **not** a dest cut, prepare close, freeze, or enroll  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Operator asked to correct farm issues after a status tick showed BigBoy
F42 (`.130`) down, `.170`/`.196` light-only, and a 4/5-node roster.

## Act

Documented BigBoy handoff (`docs/F44-BUILDER-AND-SEAT-DEPLOY.md`): F44
needs 24 GiB and cannot share XEN-BIGBOY with `mcnf-build-52` (20 GiB).
No farm slots were reserved and F44 had no `cargo`/`rustc` work.

1. `farm-slot-gc.sh --remote` on the reachable F42 nodes.
   `.170` freed ~51.4G (13G free → 44G). `.196` freed ~59.7G (9.0G → 50G).
   Both left `light-only` and became heavy-ready.
2. `xe vm-shutdown` `mcnf-build-f44`
   (`cf288dfc-301f-ae18-9b5f-1da2b1ec7704`) → halted.
   `memory-free` rose from ~4.5 GiB to ~28.7 GiB (~10 s lag).
3. `xe vm-start` `mcnf-build-52`
   (`e843193f-3b2a-8d3f-f423-4a78efef02ed`) → running.
   SSH `mm@172.20.0.130`: hostname `mcnf-build-52`, Fedora 42, rustc 1.94.0,
   cargo-generate-rpm present.
4. Slot GC on `.130` after boot: freed ~61.8G (`farm-1`, `farm-3`,
   `unpub13-full`). `/home` 79G used / 98G free → 23% used / 137G free.

The earlier status-tick `farm-enqueue.sh` prints were not a live etcd
leftover: this control host has no `/etc/mackesd/etcd-endpoints` and
`MCNF_ETCD` is unset, so `etcd_put` could not reach the mesh quorum.
`mcnf-farm-agent` is still absent on the builders; that is the slot-
dispatch farm, not a missing consumer for those no-op queue writes.

## Post-state (independent reread)

`farm-topology.sh table` and `farm-dispatch.sh nodes` at 13:16Z:

| Node | State | `/home` free | Slots |
|---|---|---|---|
| `.50` | ready | 68G | 2/2 |
| `.90` | ready | 98G | 2/2 |
| `.130` | ready | 138G | 3/3 |
| `.170` | ready | 44G | 2/2 |
| `.196` | ready | 50G | 1/1 |

**5/5 nodes up · 10/10 slots free · 0 reserved.** `.131` is halted.

## Leftover

Native F44 RPM still needs the documented handoff (halt `.52`, start
`.131`) plus `MCNF_RELEASE_INPUT_ARGV_FILE` and a complete preflight.
Do not run both BigBoy builders at once.
