# WL-FUNC-030 leftover honesty — Seat 15 + Surface live gateway probe — r1

Date: 2026-08-24  
Observed: `2026-08-24T16:25:06Z` (both seats) plus follow-up reads through
`2026-08-24T16:25:21Z`  
Classification: leftover-honesty / installed-seat live probe; **not**
set/get/clear round-trip, **not** in-place `gateway.toml` migrate,
**not** `production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at `91099e78a`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Read-only SSH. Seat 15: `mm@172.20.0.15` /
`/root/.ssh/mackes_mesh_ed25519`. Surface: `root@172.20.146.79` /
`id_ed25519`. No `seat-update-warning`. No `mde-bus request`. No
`action/voip/{set,get,clear}-gateway` publish. No invented SIP host,
username, or password. No `gateway.toml` write. Dirty control-tree
`crates/mesh/mackesd/*` files were not touched.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-030`.
- Leftover: live Bus set/get/clear plus a migrated workgroup
  `gateway.toml`.
- Prior path-only absence (2026-08-22 / 2026-08-23):
  `WL-FUNC-028-2026-08-23-installed-sync-pair-cli-r1.md`,
  `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Farm Activity contract:
  `WL-FUNC-029-030-2026-08-20-activity-admin-farm-r2.md` (fixtures do
  not close this leftover).
- Canonical file: `<workgroup_root>/voip/gateway.toml` with
  `MDE_WORKGROUP_ROOT=/mnt/mesh-storage`.
- Canonical topics: `action/voip/{set,get,clear}-gateway`, served by
  `voip_bus_responder` in the actions group.

## Installed identity (both seats)

| Field | Seat 15 `172.20.0.15` | Surface `172.20.146.79` |
|---|---|---|
| hostname | `Basement-Test-Workstation` | `SURFACE` |
| RPM | `magic-mesh-13.0.0-35.x86_64` | same |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` | same |
| `/usr/bin/mde-shell-egui` sha256 | `faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14` | same |
| `/usr/bin/mackesd` sha256 | `b27a035167946e182d65f7784384b1b54c2418e63a515cb28fcb7127cbeb1009` | same |
| Construct | pid `2353` `active` since 2026-08-23 10:27:50 EDT | pid `2516504` `active` since 2026-08-22 22:32:34 EDT |

Packed literals in the installed shell (byte counts, both seats):

| Literal | Count |
|---|---|
| `action/voip/set-gateway` | 1 |
| `action/voip/get-gateway` | 1 |
| `action/voip/clear-gateway` | 1 |
| `gateway.toml` | 2 |
| `Mesh-wide outbound registrar` | 1 |

The GUI publisher is in the installed Construct bytes. That is not a live
Bus round-trip.

## `gateway.toml` (presence only; files unread)

Canonical path and every previously cited fallback were **absent** on both
seats. `find` for `gateway.toml` under `/mnt/mesh-storage`, `/etc/mackesd`,
`/var/lib/mackesd`, `/var/lib/mde`, `/var/lib/mcnf`, `/home`, `/root`, and
`/etc` returned no paths. `/mnt/mesh-storage/voip/` does not exist.
`/mnt/mesh-storage` is a directory (`drwxr-xr-x` root:root, 62 entries,
mtime 2026-08-23 18:00); `findmnt /mnt/mesh-storage` is empty on Seat 15.
No `voip` or `gateway` name in the mesh-storage root listing. Surface
`find` for `account.toml` under `/mnt/mesh-storage` was also empty.

There is no migrated workgroup file to hydrate in place.

## Bus topics `action/voip/{set,get,clear}-gateway`

`/run/mde-bus` exists on both seats (`MDE_BUS_ROOT` unset in the SSH
session; the actions unit pins `MDE_BUS_ROOT=/run/mde-bus`). There is
**no** `/run/mde-bus/action` directory on either seat. All three topic
paths are absent:

- `/run/mde-bus/action/voip/set-gateway`
- `/run/mde-bus/action/voip/get-gateway`
- `/run/mde-bus/action/voip/clear-gateway`

`find /run/mde-bus -maxdepth 4` for `*voip*` / `*gateway*` returned no
paths. `mde-bus topic list` is a 12-row seeded registry with **zero**
`voip` / `gateway` / `voice` rows. `MDE_BUS_ROOT=/run/mde-bus mde-bus
history action/voip/get-gateway` and `…/set-gateway` exited 0 with empty
output (no retained messages). `/run/mde-bus/state/voice` is absent on
Seat 15.

## Seat 15 responder (census, not a round-trip)

`mackesd-actions.service` is `active` / `running` since 2026-08-24
12:00:10 EDT, MainPID `509158`, `mcnf-collaboration-identity` `active`,
`mackesd.target` `active`. Environment includes
`MDE_WORKGROUP_ROOT=/mnt/mesh-storage`. Thread
`/proc/509158/task/509207/comm` is `voip-bus-respon`. Journal (ISO):

```text
2026-08-24T15:34:29.176618Z VOIP gateway Bus responder spawned; serving action/voip/{set-gateway,get-gateway,clear-gateway}
2026-08-24T16:00:10.367444Z VOIP gateway Bus responder spawned; serving action/voip/{set-gateway,get-gateway,clear-gateway}
```

A spawned responder with an empty spool is not set/get/clear. Construct
opened Mesh Teams from the dock at 2026-08-24 07:24 EDT; that journal
line is not a gateway publish.

## Surface responder (not running)

`mackesd-actions`, `mackesd-control`, `mackesd-data`, and
`mackesd-integrations` are `inactive` / `dead` (no ActiveEnter).
`mackesd-compute` and `mackesd-observation` are `active` since
2026-08-24 12:18:57 EDT. `mcnf-collaboration-identity.service` is
`failed` (`Result=exit-code`, InactiveEnter 2026-08-24 12:24:51 EDT).
`mackesd-actions` `Requires=` that unit, so the voip responder never
spawned. Journal for `mackesd-actions` has no voip/gateway lines. That
identity dest is `WL-FUNC-023`, already recorded in
`WL-FUNC-023-2026-08-24-surface-collab-dest-refused-r1.md`. This probe
did not retry it.

## What this does not prove

- `set-gateway` / `get-gateway` / `clear-gateway` round-trip on a live
  Bus.
- In-place hydrate of a migrated workgroup `gateway.toml`.
- Password redaction on a live readout (no password was present or
  invented).
- Communications Activity form publish from a DRM seat.
- Dell (not in this unit's probe set).

## Blocker

Live leftover stays open. Closing it needs a real workgroup
`gateway.toml` (migrated, not invented) and an operator (or DRM harness)
set/get/clear cycle on a current-revision seat whose actions group is
up. Seat 15 has the responder and the packed GUI publisher; the spool
and the file are both absent. Surface cannot serve the topics until
actions starts. Do not invent gateway credentials. Do not flip
`production_admitted`.
