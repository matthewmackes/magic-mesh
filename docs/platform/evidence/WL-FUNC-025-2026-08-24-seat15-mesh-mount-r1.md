# WL-FUNC-025 leftover honesty — Seat 15 mesh_mount worker + Files POSIX — r1

Date: 2026-08-24  
Observed: `2026-08-24T15:47:02Z`–`2026-08-24T15:48:29Z`  
Classification: leftover-honesty / installed-seat live probe after
collaboration-identity dest admission; **not** operator Files use, **not**
local/mesh FileOps execution, **not** archive-queue round-trip, **not**
production close  
Source worktree: `agent/drain-worklist-20260725` at `7fe8fad6ccc8`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Read-only SSH except `mackesd mesh-ssh-key status` (first line only; public
line discarded). No `seat-update-warning`. No Files create/copy/compress/
extract/link. No `action/mesh-mount`. No package install. No `systemctl`
mutate. No dest invented.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-025`.
- Leftover: live seat Files (local/mesh + archive-queue).
- Prior read-only: `WL-FUNC-025-2026-08-23-live-seat-files-archive-r1.md`
  (Seat 15 then had no data-group `mesh_mount`).
- Fixtures do not close this leftover
  (`WL-FUNC-025-2026-08-23-mesh-tree-archive-queue-r1.md`).
- Context: Seat 15 dest admission
  `WL-FUNC-017-2026-08-24-seat15-collab-identity-dest-r1.md` started
  `mackesd-data` / `mackesd-integrations`.

## Seat identity

| Field | Value |
|---|---|
| Seat | `172.20.0.15` `Basement-Test-Workstation` |
| SSH | `mm@` / `/root/.ssh/mackes_mesh_ed25519` |
| RPM | `magic-mesh-13.0.0-35.x86_64` (buildtime Sat 22 Aug 2026 09:56:45 PM EDT) |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| Construct | `/usr/bin/mde-shell-egui` pid `2353` (started Sun Aug 23 10:27:50 2026 EDT), `active` |

Identical bytes vs the 2026-08-23 live probe:

```text
faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14  /usr/bin/mde-shell-egui
b27a035167946e182d65f7784384b1b54c2418e63a515cb28fcb7127cbeb1009  /usr/bin/mackesd
```

Dell `172.20.146.225` SSH: `No route to host` (twice). Surface was not
re-probed this unit.

## Data-group `mesh_mount` (now up)

`mackesd-data.service` ActiveEnterTimestamp `2026-08-24 11:34:29 EDT`
(`2026-08-24T15:34:29Z`). Journal:

```text
starting worker  worker=mesh_mount  2026-08-24T15:34:29.442383Z
```

Sibling units at probe time: `mackesd-integrations`, `mackesd-actions`,
`mackesd-control`, `mackesd-compute`, `mackesd-observation`,
`mcnf-collaboration-identity`, `mde-shell-egui`, `syncthing` all `active`.
`mcnf-wx-data` remains `inactive`.

Heartbeat topic `state/mackesd/data/workers/mesh_mount` is publishing
(78 JSON files by 15:48:28Z; generation advancing). Latest snapshot:

| Field | Value |
|---|---|
| `worker_id` | `mesh_mount` |
| `group` | `data` |
| `node_id` | `peer:Basement-Test-Workstation` |
| `state` | `running` |
| `generation` | `168` |
| `restart_count` | `0` |
| `state_reason` | none |
| `cadence` | `event_driven` |
| `state_since_ms` | `1787585669494` (= 15:34:29.494Z) |

Worker census is not a lock-11 mount and not a Files POSIX op.

## Bus topics for mesh-mount / files

- `/run/mde-bus/state/mesh-mount` **absent**
- `/run/mde-bus/action/mesh-mount` **absent**
- `/run/mde-bus/action` **absent** (zero action topics on the spool)
- `find /run/mde-bus -maxdepth 3` for `*mesh-mount*` / `*file-ops*` /
  `*opqueue*` / `*archive*`: no topic dirs (only unrelated
  `vdi-clipboard-*-files.sock`)
- `mde-bus topic list` (seeded registry): no `mesh-mount`, `files`, or
  `archive` row
- `mde-bus history state/mesh-mount --bus-root /run/mde-bus`: no topic

The worker publishes per-host `state/mesh-mount/<host>` only after a typed
`action/mesh-mount/<host>` drain. None has been requested.

## Lock-11 path and sshfs capacity

`/run/user/1000/mde-mesh` and `/run/user/0/mde-mesh` **absent**.
`findmnt -t fuse.sshfs` empty. `/run/user` has only uid `1000` (`mm`).

Host can run sshfs (`/usr/bin/sshfs`, `fuse-sshfs-3.7.6-1.fc44`,
`/dev/fuse` present). `mackesd mesh-ssh-key status` first line:
`mesh-ssh-key status: sealed` (public line not recorded). That dest is
FILEMGR-6 auth material, not a mounted tree.

`/mnt/mesh-storage` is still a directory on `/dev/mapper/fedora-root`
(`findmnt /mnt/mesh-storage` empty). Syncthing workgroup names are
present. That is the Syncthing file plane, **not** a Files mesh mount.
`~/Local` is absent for `mm` and `root`.

## Files persist and archive-queue

`~/.config/mcnf` and `~/.local/share/mcnf` are **absent** for `mm` and
`root`. No `files-folder-prefs.json`, `files-bookmarks.json`, `*opqueue*`,
`*file-op*`, or `*archive-queue*` under `/home/mm`, `/root`, `/var/lib/mde`,
`/etc/mackesd`, `/var/lib/mackesd`, `/var/lib/mcnf`.

Construct still has an in-process `mde-files-opqueue` thread (spid `4011`)
and `mde-files-previ` (spid `4012`). That is the in-memory `OpQueue`, not
on-disk archive-queue persist, and SSH cannot enqueue zip/tar.gz through it.

## No FileOps CLI / no typed POSIX bus verb

RPM `magic-mesh-13.0.0-35` ships `/usr/bin/mde-shell-egui` and
`/usr/bin/mackesd`. Zero `mde-files` / `mde-files-egui` paths in the RPM.
`command -v mde-files-egui mde-files fileops` empty.

`mackesd --help` has no Files POSIX / archive / op-queue verb. Nearby
CLIs are not substitutes:

- `mackesd mesh-fs-status` — Syncthing `/mnt/mesh-storage` df JSON
  (LizardFS-era name; not lock-11, not FileOps)
- `mackesd mesh-ssh-key` — FILEMGR-6 key lifecycle (status read only here)
- `mde-bus request` — generic `action/<domain>/<verb>`; Files POSIX ops
  are not Bus verbs. `action/mesh-mount/<host>` is mount/escalate/unmount
  and requires root DRM-shell cloud-arming, not a local-tree New File /
  Duplicate / compress / extract / link

Surface ops remain egui menubar / context actions inside pid `2353`.
SSH cannot click them. No bounded local-tree Files op was possible, so
`seat-update-warning` was not run.

## What this does not prove

- New File or Duplicate on a live local directory or a live mesh mount.
- Compress / Extract Here / Extract To zip **and** tar.gz through the
  surface queue, with progress or cancel.
- Symlink or hardlink creation with `symlink_metadata` after reload.
- Hostile refuse paths on a seat.
- Any `state/mesh-mount/<host>` Mounted path.

Farm fixture `208/208` `mde-files-egui` remains implementation evidence
only.

## Blocker

Live leftover stays open. Closing it needs operator Files use (or a DRM /
input harness that drives the Construct Files surface) on a
**current-revision** seat, including a real `action/mesh-mount/<host>` tree
and an in-surface zip **and** tar.gz queue round-trip. Seat 15 now has a
running data-group `mesh_mount` worker; that is census only. Installed
`13.0.0-35` is unpublished and older than HEAD. Dell was unreachable.
Do not invent a dest. Do not flip `production_admitted`.
