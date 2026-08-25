# WL-FUNC-025 leftover honesty — Surface Files POSIX / archive-queue — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:07Z`–`2026-08-25T10:53:59Z`  
Classification: leftover-honesty / installed-seat live probe; **not**
operator Files use, **not** local/mesh FileOps execution, **not**
archive-queue round-trip, **not** dest hydrate, **not** production close  
Source worktree: `agent/drain-worklist-20260725` at `4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
Seat: Surface `172.20.146.79` (`SURFACE`, overlay `10.42.0.7`,
`peer:SURFACE`)  
Live topology mesh-id (governed, not minted here): `mcnf-clean-20260728`  
`production_admitted: false`

Read-only SSH as `root@` with `/root/.ssh/mackes_mesh_ed25519`, plus one
non-mutating `ffmpeg -f kmsgrab` acquire. No `seat-update-warning`. No
Files create/copy/compress/extract/link. No `action/mesh-mount`. No
package install. No `systemctl` mutate. No dest JSON. No Sunshine start
(binary absent). No `seat-remote-input` / uinput. Did not SSH Seat 15 or
Dell.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-025`.
- Leftover: live seat Files (local/mesh + archive-queue).
- Prior Seat 15 census:
  `WL-FUNC-025-2026-08-24-seat15-mesh-mount-r1.md`.
- Fixtures do not close:
  `WL-FUNC-025-2026-08-23-mesh-tree-archive-queue-r1.md`.

## Seat identity

| Field | Value |
|---|---|
| Seat | `172.20.146.79` `SURFACE` overlay `10.42.0.7` |
| SSH | `root@` / `/root/.ssh/mackes_mesh_ed25519` |
| RPM | `magic-mesh-13.0.0-35.x86_64` (buildtime Mon 24 Aug 2026 03:44:23 PM EDT) |
| `mackesd --version` | `13.0.0 "Construct" · 4071ed295e18a8bd117cea5ee639eb5cafab3485 · 2026-08-24 · dev` |
| `/usr/bin/mde-shell-egui` | 60888512 bytes, mtime `2026-08-24 15:44:23` EDT |
| sha256 (shell) | `2658ea3f142750646b98798e18bb35dad6e35b3eed8c3a5cf592193e32d2fe91` |
| sha256 (mackesd) | `43cec1fb621e0e9fb9e0015ac82f1341d15ab0a1a2f0e0ebaf0987ebf4cbea54` |
| `rpm -V` shell | unmodified |
| Construct | pid `1290917` as `root`, `mde-shell-egui.service` `active` since `2026-08-24 20:25:57 EDT`, `NRestarts=0` |
| DRM | pid holds `/dev/dri/card1` (five fds); eDP-1 `2736x1824` connected/enabled |

This is dest-cut `4071ed295`, not the earlier `7e3474eeb` Seat 15 leftover
bytes. S1–S3 wiring is in that cut (HEAD of this worktree). Packed ASCII
in `/usr/bin/mde-shell-egui`: `Duplicate` 50, `Compress` 78, `Advanced` 4,
`files-folder-prefs.json` 1, `files-bookmarks.json` 1, `mde-mesh` 4,
`zip` 17. Contiguous UTF-8/UTF-16 for `New File` / `New Folder` /
`Extract Here` / `Extract To` / `Pin to Places` is absent (same shape as
the 2026-08-23 three-seat probe; missing literals are not compiled-out
verbs).

Surface has no local mesh-id file. Overlay cert name is `peer:SURFACE`.
The live workgroup id used in governed topology records remains
`mcnf-clean-20260728`; this probe did not invent one.

## kmsgrab vs GUI click

CRTC `56` pipe A mode `2736x1824` @ 60 Hz. `ffmpeg -f kmsgrab -device
/dev/dri/card1 -i - -frames:v 1 -f null -` exit 0:

```text
Template framebuffer is 141: 2736x1824 format 30335258 modifier 100000000000001 flags 2.
Stream #0:0: Video: wrapped_avframe, drm_prime, 2736x1824
frame=    1
```

`30335258` is XR30; modifier `0x100000000000001` is Intel X-tiled.
`hwdownload,format=bgr0` refused (`Invalid output format bgr0`). VAAPI
init on `renderD128` failed (`unknown libva error`). No PNG. No
readable Files control. uinput was therefore not used.

Capture dests:

| Path | State |
|---|---|
| `/usr/bin/ffmpeg` | present (8.1.2) |
| `/usr/bin/sunshine` | **absent** (package not installed) |
| grim / Moonlight | absent |
| `/usr/libexec/mackesd/seat-remote-input` | present; not invoked |
| `/dev/uinput` | present (`root:root`) |

Physical SONiX keyboard and Logitech USB mouse exist. `loginctl` seat0
`CanGraphical=yes`, empty `Sessions`. DRM-service model, not an absent
GUI.

## `mesh_mount` census (not a mount)

`mackesd-data.service` `active`. Heartbeat topic
`state/mackesd/data/workers/mesh_mount` publishing (251 JSON files at
first read; generation advancing during the probe). Latest snapshot at
`10:53:59Z`:

| Field | Value |
|---|---|
| `worker_id` | `mesh_mount` |
| `group` | `data` |
| `node_id` | `peer:SURFACE` |
| `state` | `running` |
| `generation` | `645` |
| `restart_count` | `0` |
| `state_reason` | none |
| cadence | `event_driven` |

`/run/mde-bus/state/mesh-mount` **absent**. `/run/mde-bus/action`
**absent**. `/run/user/0/mde-mesh` and `/run/user/1000/mde-mesh`
**absent**. `findmnt -t fuse.sshfs` empty. Worker census is not a
lock-11 mount and not a Files POSIX op.

`/mnt/mesh-storage` is a directory on the root LV (`findmnt` empty),
with workgroup names present (Syncthing file plane, not Files mesh
mount). `~/Local` absent for `mm` and `root`. `syncthing.service`
`active`. `mcnf-wx-data.service` `inactive`.

## Persist and archive-queue

Packaged unit `/usr/lib/systemd/system/mde-shell-egui.service` pins
`Environment=XDG_CONFIG_HOME=/root/.config` (no Seat-15-style drop-in).
Live pid environ: `USER=root`, `XDG_CONFIG_HOME=/root/.config`,
`XDG_RUNTIME_DIR=/run/user/1000`, **no `HOME`**. Resolver path would be
`/root/.config/mcnf/files-folder-prefs.json`. `/root/.config/mcnf` is
**absent**. No `files-folder-prefs.json`, `files-bookmarks.json`, or
op-queue persist. Dest JSON was not written.

Construct has in-process `mde-files-opqueue` (spid `1292437`) and
`mde-files-previ` (spid `1292438`). That is the in-memory `OpQueue`, not
an on-disk archive-queue round-trip. SSH cannot enqueue zip/tar.gz
through it.

## No FileOps CLI

RPM ships `/usr/bin/mde-shell-egui` and `/usr/bin/mackesd`. No
`mde-files` / `mde-files-egui` / `fileops` binaries. `mackesd --help`
has no Files POSIX / archive / op-queue verb. Nearby `mesh-fs-status` is
Syncthing `df`, not FileOps. Surface ops remain egui menubar / context
actions inside pid `1290917`.

## What this does not prove

- New File or Duplicate on a live local directory or a live mesh mount.
- Compress / Extract Here / Extract To zip **and** tar.gz through the
  surface queue, with progress or cancel.
- Symlink or hardlink creation with `symlink_metadata` after reload.
- Hostile refuse paths on a seat.
- Any `state/mesh-mount/<host>` Mounted path.
- A painted Files frame (kmsgrab acquired tiled XR30; no PNG).

## Blocker

Live leftover stays open. Closing it needs operator Files use (or a DRM
input harness that clicks a **visible** Files control after a linear /
EGL-readback frame) on this current-revision seat, including a real
`action/mesh-mount/<host>` tree and an in-surface zip **and** tar.gz
queue round-trip. Dest hydrate is not that leftover. Do not invent a
dest. Do not flip `production_admitted`.
