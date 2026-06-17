# HA — QNM-Shared shadow master + 2-lighthouse minimum

Design survey locked 2026-06-17 (10-question `/plan`), prompted by the QNM-Shared
outage where the single LizardFS master (a 947 MB droplet) died and wedged the
whole mesh. The QNM-Shared master is a **SPOF**; this epic adds a live **shadow
master** with **automatic, fenced failover** over a **floating overlay VIP**, and
makes **2 lighthouses a minimum** for a healthy mesh.

## Locks (10)

| # | Decision | Lock |
|---|----------|------|
| Q1 | Topology | **Keep `45.55.33.179` (10.42.0.1) master; `159.65.183.51` (10.42.0.2) shadow** — no migration |
| Q2 | Mechanism | **Shadow master (`PERSONALITY=shadow`, live replication, promotable) + metalogger** |
| Q3 | Failover | **Automatic with fencing** |
| Q4 | Client addressing | **Floating overlay VIP** — clients mount the VIP; failover moves it → no remount |
| Q5 | Failback | **Stay** — recovered master rejoins as the new shadow (no flapping) |
| Q6 | Fence | **QNM leader-lease** — only the mackesd leader-lease holder may promote/claim the VIP (a partitioned shadow can't self-promote) |
| Q7 | 2-LH minimum | **Warn at founding + a `degraded: no HA` health flag** until a 2nd lighthouse exists (non-blocking, visible) |
| Q8 | Shadow storage | **Yes — shadow also runs a chunkserver** (keeps a data replica) |
| Q9 | Metadata sync | **Live shadow replication** (RPO ≈ moment of failure) |
| Q10 | Surface | **mackesd `ha` worker + Workbench Mesh Storage panel + failover bus alert** |

## Architecture

```
   clients mount  -H <VIP 10.42.0.100>  (floating overlay VIP)
                         │
          ┌──────────────┴───────────────┐
   45.55.33.179 / 10.42.0.1        159.65.183.51 / 10.42.0.2
   PERSONALITY=master  ◄──live metadata stream──  PERSONALITY=shadow
   holds the VIP                                  + metalogger (live backup)
   + chunkserver                                  + chunkserver
                         │
          mackesd `ha` worker on BOTH lighthouses:
            • watches master liveness (VIP reachable + mfsmaster alive)
            • FENCE = QNM-Shared leader-lease — only the lease holder promotes
            • on master-down: lease-holding shadow → promote (shadow→master)
              + claim the VIP  (no auto-failback; old master returns as shadow)
            • publishes HA state + fires a failover alert
```

- **Single-master invariant:** the floating VIP + the QNM leader-lease together
  guarantee one master. A shadow that loses the overlay/lease (partition) cannot
  claim the VIP and will not promote → no split-brain on a 2-node setup.
- **Transparent failover:** clients mount the VIP (not a node IP), so when the VIP
  moves to the promoted shadow the existing FUSE mount keeps working (LizardFS
  reconnects to the same address).
- **2-lighthouse minimum:** founding a mesh warns if only 1 lighthouse exists, and
  mesh health reports `degraded: no HA` until a 2nd is enrolled — so a single-LH
  mesh is allowed but loudly flagged as non-resilient.

## Tasks → HA-1..HA-6 (see WORKLIST)

## Acceptance (runtime-observable)
- Kill mfsmaster (or power off the master): within the failover window the shadow
  (holding the leader-lease) promotes to master + claims the VIP; clients keep
  their mounts and QNM-Shared stays readable mesh-wide; a failover alert appears.
- A partitioned shadow that cannot reach the leader-lease does NOT promote (no
  split-brain).
- Bring the old master back: it rejoins as the shadow (no auto-failback, no flap).
- Founding with 1 lighthouse → `degraded: no HA`; enrolling a 2nd clears it.
- Mesh Storage panel shows master/shadow roles + replication state + last failover.
- Both lighthouse `/tmp` tmpfs sized down so a heavy transient can't OOM the
  master (HA-6, pairs with the netdata RAM gate).

## Risks
- **Split-brain** if both the VIP and the lease fence fail — mitigated by requiring
  BOTH (VIP exclusivity + lease) to promote.
- **Master fragility** (947 MB) — HA reduces blast radius, but pair with the tmpfs
  shrink + the netdata RAM gate so the master isn't tipped in the first place.
- **VIP on the overlay** — must be a free 10.42.0.x not assigned to any node;
  arping/GARP semantics over Nebula (point-to-point) — validate VIP reachability
  after a move.

## Out of scope (v1)
- 3+ master quorum / Raft (2-LH + lease-fence is the target).
- Auto-failback (Q5 = stay).
- Historical metadata snapshots (Q9 = live-only; a metalogger backup exists but
  point-in-time restore is a later add).
