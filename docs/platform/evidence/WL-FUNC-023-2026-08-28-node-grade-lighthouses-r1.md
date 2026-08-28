# WL-FUNC-023 — node_grade counts listed lighthouse IPs (2026-08-28)

Source heal for Health `lighthouse-unreachable` / `reachable_lighthouses: 0`
when overlay ping to listed lighthouse IPs succeeds but `nodes[]` has no
lighthouse rows. `production_admitted` unchanged. No live-seat mutation.
Dest IPs were not invented.

## Bug

`reachable_lighthouse_count()` only counted `nodes[]` rows with
`role == "lighthouse"`. Mesh-status used to omit those rows when
`/mnt/mesh-storage` was not a mount, so grade F persisted while
`network.lighthouse_ips` already held the overlay IPs from nebula
`static_host_map` keys.

## Source change (`crates/mesh/mackesd/src/workers/node_grade.rs`)

When `nodes[]` omit lighthouse rows, count `network.lighthouse_ips`. Prefer
node presence when a row matches; unmatched listed IPs count as reachable
if the overlay is up.

## Verification (farm)

BigBoy `.130` hung compiling (host later unreachable). Warm retry:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd node_grade -- --test-threads=1
```

Exit 0. Focused lib: `42 passed, 0 failed`, including
`reachable_lighthouses_use_listed_ips_when_nodes_omit_lighthouse_rows`.

This is source evidence only. Installed-seat Health after packaging remains
a live leftover on `WL-TEST-003` after a testing Beta.
