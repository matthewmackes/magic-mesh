# WL-FUNC-023 — mesh-status lighthouse rows from static_host_map (2026-08-28)

Source heal for Health honesty on seats where overlay ping to `10.42.0.1`–`.3`
succeeds but `/run/mde/mesh-status.json` `nodes[]` listed only workstations.
`production_admitted` unchanged. No live-seat mutation. `mackesd` / desktop
crates were not touched. Dest, token, and mesh-id were not invented.

## Bug

`node_grade` counts `reachable_lighthouses` from `nodes[]` rows with
`role == "lighthouse"` and presence not `offline`/`unreachable`. The snapshot
already parsed lighthouse overlay IPs into `network.lighthouse_ips` from
nebula `static_host_map` KEYS (`NET_LHIPS`), but `nodes[]` was filled only
from etcd peers or Syncthing `shell-status.json` under `/mnt/mesh-storage`.
When that path is an unmounted leftover directory, lighthouse shell-status
never appears, so Health reports `lighthouse-unreachable` /
`reachable_lighthouses: 0` while overlay ICMP to the same IPs works.

## Source change (`install-helpers/mesh-status-snapshot.sh`)

After workstation aggregation, the snapshot injects one `nodes[]` row per
`NET_LHIPS` overlay IP (static_host_map keys only — public ip:port VALUES
are not used, and no IP is invented):

```json
{
  "hostname": "10.42.0.1",
  "overlay_ip": "10.42.0.1",
  "role": "lighthouse",
  "presence": "online",
  "last_seen_ms": 1787900000000,
  "version": null,
  "services": {}
}
```

- **hostname** is the overlay IP when no existing peer row carries that
  address. An existing etcd/fs row with the same overlay IP is retagged
  `role=lighthouse` instead of duplicated (hostname/services/version kept).
- **presence** is `online` when `ping -c1 -W1` (bound to the nebula iface
  when known) to that overlay IP succeeds; otherwise `unreachable`.
- Workstation rows from etcd/fs stay. Injection does not require
  `/mnt/mesh-storage`. Empty `NET_LHIPS` adds nothing.

## Verification (local bash; no cargo)

```text
bash -n install-helpers/mesh-status-snapshot.sh
install-helpers/mesh-status-snapshot.sh --self-test
```

Both passed. `--self-test` proves: keys `10.42.0.1,10.42.0.2,10.42.0.3` from a
fixture `static_host_map` (VALUES `203.0.113.10` / `198.51.100.2` / `192.0.2.3`
are not leaked); a stub ping maps `.1`/`.3` → `online` and `.2` →
`unreachable`; workstation aggregation is kept; a same-IP server row is
retagged not duplicated; empty `NET_LHIPS` invents no rows; aggregator
python compiles.

This is source evidence only. Installed-seat Health after the helper is
packaged remains a live leftover; this note does not close `WL-FUNC-023`.
