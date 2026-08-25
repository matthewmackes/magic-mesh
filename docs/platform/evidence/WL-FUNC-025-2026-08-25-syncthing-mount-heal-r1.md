# WL-FUNC-025/026/027 — Syncthing helper mount-heal (source) — r1

Date: 2026-08-25  
Observed: `2026-08-25T15:20:00Z`  
Classification: leftover-heal / source + helper self-test; **not** live
seat mutation, **not** dest hydrate, **not** production close  
Source worktree: `agent/drain-worklist-20260725` at
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

No SSH to Seat 15, Dell, or Surface. No `seat-update-warning`. No
Sunshine. No wipe. No bind or block mount of production
`/mnt/mesh-storage`. No dest invented.

## Authority

- Worklist leftovers: `WL-FUNC-025` / `WL-FUNC-026` / `WL-FUNC-027`
  live-seat (Files POSIX / folder-prefs / bookmarks). Shared blocker:
  Surface `/mnt/mesh-storage` is a directory on the root LV, not a
  mount (`WL-FUNC-025-2026-08-25-surface-files-posix-r1.md`).
- Health already fail-closes the same way in `node_grade`
  (`mesh_storage_mounted` / XDG Downloads use `mountpoint -q`).
- This unit is the packaged helper `setup-syncthing.sh`. It previously
  `mkdir -p` the folder and logged `done` without asking whether the
  path was a mount.

## What the helper did before

`install-helpers/setup-syncthing.sh` did **not** create a bind or real
mount. SUBSTRATE-V2 comments called `/mnt/mesh-storage` a plain
directory (no FUSE). The live path was:

```text
mkdir -p "$HOME_DIR" "$FOLDER"
… configure syncthing …
log "done — folder $FOLDER shared full-mesh …"
```

An empty directory on `/` therefore counted as a healthy file plane.
That is the Surface leftover: `findmnt /mnt/mesh-storage` empty,
Syncthing still `active`.

## What changed

- Preflight `require_mesh_folder_mount` runs **before** package install
  or config write. It requires `[ -d "$FOLDER" ] && mountpoint -q
  "$FOLDER"`. Missing `mountpoint`, a missing path, or a non-mount
  directory exits 1 with `is not a mountpoint` and never logs `done`.
- The helper still does not invent a dest and still does not mount
  production `/mnt/mesh-storage`. A real or bind mount must already
  exist. Idempotent re-run on an already-mounted folder is unchanged
  (identity, overlay-only XML, units, reconcile timer).
- `mkdir -p "$FOLDER"` is gone so the helper cannot mint an empty dir
  and then claim it ready. `mkdir -p "$HOME_DIR"` remains (config home).
- `--self-test` exercises a real non-mount temp dir (must fail), a
  missing path (must fail), and a PATH-mocked successful `mountpoint
  -q` (must pass). Temp fixtures only; never `/mnt/mesh-storage`.
- Existing `install-helpers/test-syncthing-device-scope.sh` mocks
  `mountpoint` for the prior device-scope cases and adds
  `run_setup_non_mount_fails` so a non-mount `--folder` cannot reach
  `dnf` / `syncthing generate` / `systemctl` / `done`.

Farm cargo was not required (shell only).

## Self-test result (local, this host)

```text
$ install-helpers/setup-syncthing.sh --self-test
setup-syncthing: self-test passed

$ bash install-helpers/test-syncthing-device-scope.sh
ok: non-mount directory fails closed and is not claimed ready
ok: full setup prunes only stale unshared globals from an authoritative registry
ok: failed and empty registry reads preserve folder shares and global devices
ok: health ignores stale/unshared globals and accepts all connected folder peers
ok: health alerts 1/2 when a real managed-folder peer is disconnected
ok: health leaves etcd alone when any coordination endpoint is healthy
ok: stale own-peer publication fails health and requests bounded observation recovery
ok: timer reconciler adds missing global/folder membership without restart or deletion
ok: offline timer reconciler performs no Syncthing mutation
ok: amplified registry input is capped before it can amplify Syncthing CLI mutations
PASS: Syncthing managed-folder device-scope self-test
```

A non-mount directory now fails closed. Fixtures do not close the live
leftover.

## Remaining live leftover

Surface `172.20.146.79` still has `/mnt/mesh-storage` as a directory on
the root LV (`WL-FUNC-025-2026-08-25-surface-files-posix-r1.md`). This
source change does not remount that seat. Closing the file-plane part
needs an **authorized dest** that provides a real or bind mount at
`/mnt/mesh-storage` (not invented here), then a packaged helper re-run
on that mount. WL-FUNC-025/026/027 still also need operator Files use
(POSIX / view-prefs / pin) on a current-revision seat. Do not flip
`production_admitted`.
