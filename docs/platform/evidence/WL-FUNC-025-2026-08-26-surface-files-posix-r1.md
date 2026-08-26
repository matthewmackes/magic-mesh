# WL-FUNC-025 leftover — Surface live Files POSIX (local FileOps/OpQueue) — r1

Date: 2026-08-26  
Observed: `2026-08-26T11:53:00Z`–`2026-08-26T11:59:50Z`  
Classification: leftover live-seat FileOps/OpQueue execution on local xfs;
**not** Construct menubar/context click, **not** pid-3653 in-process
`mde-files-opqueue`, **not** lock-11 mesh mount, **not** dest hydrate,
**not** production close  
Source worktree: `agent/drain-worklist-20260725` at
`8e6f84a2ecbd644e97572020c94e4959258b65b5` (Files crates identical to
dest-cut)  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c`  
Control host: `rocky9-kvm2`  
Seat: Surface `172.20.146.79` (`SURFACE`, overlay `10.42.0.7`,
`peer:SURFACE`)  
Live topology mesh-id (governed, not minted here): `mcnf-clean-20260728`  
`production_admitted: false`

SSH as `root@` with `/root/.ssh/mackes_mesh_ed25519`. Packaged
`/usr/libexec/mackesd/seat-update-warning` at `11:58:42Z` (toast persisted
ULID `01M0YYZNQVXEVHDZQV8YQ5VCKC`; broker `10.42.0.7:8443` unreachable).
Waited five seconds, then mutated `/root/Local/wl-func-025-live` and
`/tmp/mcnf-func025-bins` only. No Sunshine (binary absent). No uinput. No
kmsgrab. No `action/mesh-mount`. No package install. No `systemctl`
mutate. Did not confirm Restart mackesd. Did not flip
`production_admitted`. Did not invent a dest. Did not SSH Seat 15 or Dell.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-025`.
- Leftover: live seat Files (local/mesh + archive-queue).
- Prior census (not a POSIX op):
  `WL-FUNC-025-2026-08-25-surface-files-posix-r1.md`.
- Farm fixture (not this leftover):
  `WL-FUNC-025-2026-08-23-mesh-tree-archive-queue-r1.md`.
- Dest-cut record:
  `WL-FUNC-023-2026-08-25-destcut-bc14a22d7-r1.md`.

## Dest-cut (verified, not invented)

| Field | Value |
|---|---|
| Seat | `172.20.146.79` `SURFACE` overlay `10.42.0.7` |
| SSH | `root@` / `/root/.ssh/mackes_mesh_ed25519` |
| RPM | `magic-mesh-13.0.0-35.x86_64` buildtime Tue 25 Aug 2026 11:33:54 AM EDT |
| `mackesd --version` | `13.0.0 "Construct" · bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c · 2026-08-25 · dev` |
| sha256 (shell) | `955d8d0a67de750b4572134b1fb9ab35ef259e287270170e87ee16352c262bb2` |
| sha256 (mackesd) | `9c3a5a0212136b8e893d73f95c5f6dd0fa907bb371ea989758a48b97192f16a2` |
| `rpm -V` shell | unmodified |
| Construct | pid `3653` as `root`, `mde-shell-egui.service` `active` since `2026-08-26 06:45:07 EDT`, `NRestarts=0` (unchanged after this probe) |
| DRM | pid holds `/dev/dri/card1` (five fds) |
| Unit pin | packaged `Environment=XDG_CONFIG_HOME=/root/.config` |
| Overlay cert | `peer:SURFACE` `10.42.0.7/17` |

This is dest-cut `bc14a22d7`, newer than the 2026-08-25 leftover's
`4071ed295`. `git diff` of dest-cut vs HEAD for
`dialogs.rs` / `menubar.rs` / `opqueue.rs` / `fileops.rs` / `archive.rs`
is empty — source wiring is complete vs dest-cut. No crate edit in this
unit. Packed ASCII in `/usr/bin/mde-shell-egui`: `Duplicate` 50,
`Compress` 78, `Advanced` 4, `zip` 9. Contiguous UTF-8/UTF-16 for
`New File` / `Extract Here` / `Extract To` / Create Link labels is
absent (same shape as prior probes; missing literals are not compiled-out
verbs). No `/usr/bin/mde-files` / `mde-files-egui` FileOps CLI.

## Farm (not re-run)

`.50` / `.90` saturated; `.170` free but `FULL(disk)` 7.4G. Identical
commands already passed at `b6fd8aeab-dirty` on `.130` slot 1 (Files
sources unchanged through `8e6f84a2e`). Did not hand-duplicate
(§4A.5).

| Job | Command | Result |
|---|---|---|
| `a1183c2d3475` | `cargo test -p mde-files-egui` | pass, 210 tests, 0 failed |
| `edd4ab8d84fb` | `cargo test -p mde-files` | pass, 159 lib + 5 integration, 0 failed |

Including `new_file_duplicate_and_links_execute_on_a_mesh_mounted_tree`
and `zip_and_tar_gz_round_trip_through_the_queue`.

## Which of the six ops executed

Farm-built test binaries from `.130` `magic-mesh-farm-d1`
(`mde_files-e966e534f5d8a0b8`, `mde_files_egui-2dde4a99848aa944`) were
copied to Surface `/tmp/mcnf-func025-bins` after the alert and run with
`TMPDIR=/root/Local/wl-func-025-live` (root LV xfs, device `64512`, never
mesh-synced `~/Local`). Binaries removed afterward.

### Local (root xfs `/root/Local`) — all six

| Op | Through the engine on Surface | Notes |
|---|---|---|
| New File | **yes** | `LiveFileOps::create_file` (`O_CREAT\|O_EXCL`); FileBrowser `submit_name_dialog` |
| Duplicate | **yes** | `LiveFileOps::duplicate` and FileBrowser `OpKind::Copy` `name (copy)` |
| Compress | **yes** | zip via `archive::compress` + FileBrowser `OpKind::Compress` zip **and** tar.gz |
| Extract | **yes** | zip live round-trip; FileBrowser extract-to zip **and** tar.gz; path-traversal refuse; cancel leaves no half-archive |
| Symlink | **yes** | `LiveFileOps::symlink`; FileBrowser Advanced create; `symlink_metadata` after reload in the FileBrowser test |
| Hardlink | **yes** | `LiveFileOps::hard_link`; FileBrowser create; non-regular refuse |

`mde-files` live tests: 5 passed / 0 failed (154 filtered).  
`mde-files-egui` FileBrowser/OpQueue tests: 6 passed / 0 failed (204
filtered), including existing-name, read-only parent, mesh-escape
symlink refuse, and cancel-compress.

That OpQueue is the test-bin `FileBrowser` worker, **not** Construct
spid `mde-files-opque` (task `5290` of pid `3653`). No menubar click.

### Mesh (lock-11) — none of the six

`/run/user/0/mde-mesh` and `/run/user/1000/mde-mesh` absent.
`findmnt -t fuse.sshfs` empty. `/run/mde-bus/state/mesh-mount` absent.
`/mnt/mesh-storage` is still a directory on the root LV (`findmnt`
empty), not a Files mesh mount. The FileBrowser "mesh-mounted tree" test
used a **fixture-shaped**
`…/run/user/1000/mde-mesh/oak/docs` path on local xfs. That is not
sshfs and does not close the mesh leftover. No `action/mesh-mount`
(cloud-arm is the root DRM shell; this SSH session did not invent a
mount dest).

### Durable listing (syscall twin, after engine tests)

Engine tests clean their tempdirs. A Python LiveFileOps-equivalent then
left `/root/Local/wl-func-025-live/durable/` for `lstat` evidence:
empty `empty.txt`; `notes (copy).txt`; zip/tar.gz extract-here/extract-to
`notes.txt`; `notes.link` → `notes.txt` (`S_ISLNK`); `notes.hard` same
inode as `notes.txt` (`nlink=2`). Existing-name `EEXIST`. Cross-device
hardlink `/tmp` (tmpfs) → `/root/Local` (xfs) `EXDEV` errno 18,
destination absent. Traversal members `../etc/passwd`, `/etc/passwd`,
`ok/../../x` classified as escapes. These durable files are **not** the
Construct queue.

## What this does not prove

- Construct Files menubar / context click on pid `3653`.
- zip/tar.gz through the in-process `mde-files-opqueue` thread.
- Any of the six ops on a live `state/mesh-mount/<host>` Mounted path.
- A painted Files frame.

## Leftover status

**Remaining.** Local FileOps + FileBrowser OpQueue executed all six on
this current-revision seat. Closing the epic still needs a real lock-11
mesh tree and operator (or visible-control) Files use inside Construct.
Do not flip `production_admitted`.
