# WL-FUNC-023 leftover — lighthouse overlay and etcd recovered (2026-08-24)

Operator authorized destructive correction of seats and lighthouses, with
DigitalOcean and DHCP access. Red `AI-GENERATED-ALERT` and five-second hold
before each mutation. Mesh-id unchanged: `mcnf-clean-20260728`.
`production_admitted` unchanged. No droplet replaced (quorum preserved).

## Cause

All three `magic-lighthouse` droplets were `active` on DigitalOcean. Overlay
LH2 (`10.42.0.2`) ↔ LH3 (`10.42.0.3`) worked. LH1 (`10.42.0.1`,
`lh-mcnf-clean-20260728-1785239652`) was partitioned:

- `/run` tmpfs 100% full (`/run/mde-bus` ~189 MiB, tens of thousands of
  `.tmp` files plus `index.sqlite-wal`).
- Nebula pid 902 wedged: UDP `4242` Recv-Q ~189 KiB, handshake timeouts,
  reload storms from `mackesd` `nebula_supervisor` `reload-or-restart`.
- etcd on LH1 could not commit; majority `.2`/`.3` stayed healthy.

LH2 and LH3 `/run` were likewise full; they were pruned after LH1 overlay
returned, one node at a time, without stopping etcd or Nebula.

## Corrected-forward

1. Stop grouped `mackesd` on LH1, reclaim `/run/mde-bus`, force-restart the
   wedged Nebula process. Overlay pings LH1→`.2`/`.3` returned (~83 ms / ~69 ms).
2. Raise live etcd cgroup `MemoryMax` from 128M to 256M (RSS was at the old
   cap). Persist `/etc/systemd/system/etcd.service.d/30-catchup-headroom.conf`.
3. Start LH1 grouped `mackesd`. Three-member etcd health: all commit.
4. Prune LH2 then LH3 `/run/mde-bus` (thin `mackesd.service`). `/run` now
   ~18–25% on each lighthouse.

Seat 15 (`10.42.0.5`) and Surface (`10.42.0.7`) overlay ping LH1, LH2, and
each other. Control-host TCP 4242 remaining closed is expected (Nebula is
UDP).

## Result

Lighthouse overlay and etcd quorum are up. FUNC-023 leftover (3) enroll on
Dell is still blocked on power/SSH (separate evidence). Surface collaboration
dest is a sibling unit.
