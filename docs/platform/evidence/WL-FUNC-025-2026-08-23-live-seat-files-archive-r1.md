# WL-FUNC-025 leftover honesty — live seat Files (local/mesh + archive-queue) — r1

Date: 2026-08-23  
Observed: `2026-08-24T00:02:49Z`–`2026-08-24T00:04:00Z`  
Classification: leftover-honesty / installed-seat read-only probe; **not**
operator Files use, **not** local/mesh FileOps execution, **not** archive-queue
round-trip, **not** production close  
Source worktree: `agent/drain-worklist-20260725` at `f5362d865`  
Installed seats: unpublished `magic-mesh-13.0.0-35` / `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Read-only SSH. No `seat-update-warning.sh`. No Files create/copy/compress/
extract/link. No `action/mesh-mount`. No package install. No `systemctl`
mutate.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-025`.
- Leftover: live seat Files (local/mesh + archive-queue).
- Fixtures do not close this leftover
  (`WL-FUNC-025-2026-08-23-mesh-tree-archive-queue-r1.md`).

## Installed identity (all three acceptance seats)

| Seat | Address | hostname | SSH | RPM | `mackesd --version` |
|---|---|---|---|---|---|
| Dell | `172.20.146.225` | `DELL-LAPTOP` | `mm@` / `mackes_mesh_ed25519` | `magic-mesh-13.0.0-35.x86_64` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| Seat 15 | `172.20.0.15` | `Basement-Test-Workstation` | `mm@` / same key | same RPM | same CLI family |
| Surface | `172.20.146.79` | `SURFACE` | `root@` / `id_ed25519` | same RPM | same CLI family |

RPM buildtime on each seat: `Sat 22 Aug 2026 09:56:45 PM EDT`. Identical
bytes:

```text
faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14  /usr/bin/mde-shell-egui
b27a035167946e182d65f7784384b1b54c2418e63a515cb28fcb7127cbeb1009  /usr/bin/mackesd
```

`mde-shell-egui.service` is `active` and `/usr/bin/mde-shell-egui` is running
on all three (Dell pid `2055123`, Seat 15 pid `2353`, Surface pid `2516504`;
all root). There is no separate `/usr/bin/mde-files` or `/usr/bin/construct`.

## Current-tree vs installed `13.0.0-35`

`7e3474ee` is an ancestor of worktree `f5362d865`. S1–S3 menubar / context /
dialog wiring is already in the installed SHA
(`crates/desktop/mde-files-egui/src/{menubar.rs,view.rs,dialogs.rs}` and
`crates/services/mde-files` have **zero** diff vs HEAD). POSIX landing
`c9edb2be9` and read-only-parent `56228211a` are ancestors of the RPM.

Current-tree-only Files hunks after the RPM:

| Commit | What it is | Live leftover? |
|---|---|---|
| `43459f809` | fixture mesh-path + zip/tar.gz queue tests | no — farm fixture |
| `57094e4b9` | folder-prefs slug/absolute alias keys | no — FUNC-026 persist, not POSIX ops |

So installed `13.0.0-35` already carries the same reachable File / Advanced /
context-menu command set as current-tree. The leftover is **execution on a
live local tree, a live mesh-mounted tree, and the in-process archive
queue**, not missing source wiring.

Release rodata is not a second source of truth. The installed shell contains
ASCII `Duplicate`, `Compress`, `Advanced`, `files-folder-prefs.json`,
`files-bookmarks.json`, `mde-mesh`, and `zip`. Contiguous UTF-8 for
`New File`, `New Folder`, `Extract Here`, `Extract To`, and the two Create
Link labels is absent (UTF-16 also absent). `New Folder` is the pre-025
command and is equally absent, so missing literals are **not** evidence that
those verbs were compiled out.

## Live Files persist and archive-queue

No `files-folder-prefs.json` and no `files-bookmarks.json` under
`~/.config/mcnf/`, `~/.local/share/mcnf/`, `/var/lib/mde/`, or
`/etc/mackesd/` on any seat. `~/.config/mcnf` itself is absent for `mm`
(Dell, Seat 15) and for Surface `root` / `mm`.

`OpQueue` is in-process (`mde-files-opqueue` thread). No on-disk archive
queue, no `*opqueue*` / `*file-op*` persist, and no leftover zip/tar.gz
produced by Files. Seat 15 `find` hits under `~/browser-vm-review/` are icon
assets, not a queue.

There is no Files CLI. Surface ops are egui menubar / context actions inside
the running Construct process. SSH cannot click them.

## Live local vs mesh tree

`/mnt/mesh-storage` exists on all three as a **directory on the root LV**
(`findmnt` empty; `df` shows `/dev/mapper/fedora-root`). Workgroup names
(`adfilter`, `bookmarks`, `Basement-Test-Workstation`, …) are present. That
is the Syncthing file plane, **not** a lock-11 Files mesh mount.

Lock-11 path `/run/user/<uid>/mde-mesh/<host>` is **absent** on every seat
(`uid 1000` and Surface `uid 0`). `findmnt -t fuse.sshfs` is empty. No
`state/mesh-mount/<host>` topics. `mde-bus list` has no `mesh-mount` or
`files` row.

`mesh_mount` worker (data group, event-driven):

| Seat | data-group `mackesd serve` | `state/mackesd/data/workers/mesh_mount` |
|---|---|---|
| Dell | yes (`mcnf-wx-data.service`) | heartbeat `state: running`, `node_id: peer:DELL-LAPTOP`, generation `7658` — census only, no mounted host |
| Surface | yes (`mcnf-wx-data.service`) | heartbeat `state: running`, `node_id: peer:SURFACE`, generation `7649` — same |
| Seat 15 | **no** (`mackesd-data` / `mcnf-wx-data` inactive; no `mackesd serve`) | worker dir **absent** |

`~/Local` exists only on Dell (`mcnf-func028-src` / `mcnf-func028-dst` from
FUNC-028). Seat 15 and Surface have no `~/Local`. Those FUNC-028 dirs are
not Files New File / Duplicate / archive-queue evidence.

Syncthing: Seat 15 `systemctl is-active syncthing` = `active` (two
`syncthing serve --home=/var/lib/mcnf-syncthing` processes). Dell and
Surface reported `activating` at probe time. None of that mounts
`/run/user/*/mde-mesh`.

## What this does not prove

- New File or Duplicate on a live local directory or a live mesh mount.
- Compress / Extract Here / Extract To zip **and** tar.gz through the
  surface queue, with progress or cancel.
- Symlink or hardlink creation with `symlink_metadata` after reload.
- Hostile refuse paths on a seat (existing name, read-only parent,
  path-traversal member, cross-device hardlink, mesh-escape symlink).

Farm fixture `208/208` `mde-files-egui` remains implementation evidence
only.

## Blocker

Live leftover stays open. Closing it needs operator Files use (or a DRM /
input harness that drives the Construct Files surface) on a
**current-revision** seat, including a real `action/mesh-mount/<host>` tree
and an in-surface zip **and** tar.gz queue round-trip. Installed
`13.0.0-35` is unpublished and older than HEAD; Seat 15 additionally has no
data-group `mesh_mount`. There is no Files CLI to substitute. Do not invent
a dest. Do not flip `production_admitted`.
