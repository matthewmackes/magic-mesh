# WL-FUNC-026 leftover — Seat 15 folder-prefs persist dest — r1

Date: 2026-08-24  
Observed: `2026-08-24T15:47:28Z`–`2026-08-24T15:50:14Z`  
Classification: leftover dest / live restart hydrate; **not** operator GUI
view/sort/hidden mutation, **not** `production_admitted`, **not** a package
install  
Source worktree: `agent/drain-worklist-20260725` at `59ed89e35`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
Seat: `172.20.0.15` `Basement-Test-Workstation`  
`production_admitted: false`

Red `AI-GENERATED-ALERT` via `/usr/libexec/mackesd/seat-update-warning` on
Seat 15 (broker persist `--no-broker`; first `WARN_RC=0` at
`2026-08-24T15:49:00Z`, ULID `01M0T7BQQJ77H9JZRX0F9T4N7S`). Five-second hold
before mutation. Later restarts used the same helper (`WARN_RC=0`). No
secrets recorded. No GUI clicks. No capture dest.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-026`.
- Leftover: live restart evidence (fixtures do not satisfy production).
- Prior absence: `WL-FUNC-025-2026-08-23-live-seat-files-archive-r1.md`,
  `WL-FUNC-028-2026-08-23-installed-sync-pair-cli-r1.md`.

## Path used

`mde-files-egui` `default_config_file("files-folder-prefs.json")` is
`$XDG_CONFIG_HOME/mcnf/files-folder-prefs.json`, else
`$HOME/.config/mcnf/files-folder-prefs.json` (`None` if both unset).

The root DRM unit did not set `HOME` or `XDG_CONFIG_HOME`. Live pid `2353`
environ had `USER=root` and `XDG_RUNTIME_DIR=/run/user/1000` only, so the
store path was `None` and GUI persist could never write. That is why the
file was absent on 2026-08-23.

Dest that makes the in-tree resolver work:

| Item | Value |
|---|---|
| Drop-in | `/etc/systemd/system/mde-shell-egui.service.d/60-xdg-config.conf` |
| Env | `XDG_CONFIG_HOME=/root/.config` (not `HOME`) |
| Store | `/root/.config/mcnf/files-folder-prefs.json` |
| Mode | `0600` regular file, `nlink=1`, not a symlink |
| Size | 479 bytes (cap 256 KiB / 256 entries) |
| SHA-256 | `4d905a5e1f9e21126198f0171f846a1c07d5f92cd6ea8862cbdc0ce1ee52bd1b` |

Bounded dest body (two entries; Grid+Modified+hidden on `/home/mm`,
Details+Name on `local:docs`).

## Restart

`mde-shell-egui.service` restarted after the dest. Identical SHA and inode
across three restarts (pids `2353` → `472289` → `474600` → `476582`). Drop
of a process that had already reached `surfaces` did **not** rewrite the
store (hydrate leaves disk untouched until a mutation).

Installed identity unchanged:

- RPM `magic-mesh-13.0.0-35.x86_64`
- `/usr/bin/mde-shell-egui` SHA-256
  `faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14`
- Journal: `mde-shell-egui starting` `13.0.0 · 7e3474eeb` `drm:true`

## Hydrate (observable)

`FileBrowser` is built at boot milestone `surfaces` (`Shell::new_for_ctx` →
`real_browser()`). Pid `476582`:

- `starting` `2026-08-24T15:50:07Z`
- `seat` `2026-08-24T15:50:08Z`
- `surfaces` `2026-08-24T15:50:13Z`

inotify on the dest file at that construction:

```text
OPEN ACCESS CLOSE_NOWRITE  /root/.config/mcnf/files-folder-prefs.json
```

No `MODIFY` / `CLOSE_WRITE`. SHA after hydrate matched SHA before restart.
New process environ: `XDG_CONFIG_HOME=/root/.config`. Unit `active`,
MainPID `476582`.

## What this does not prove

- Operator Files toolbar/menu changed view, sort, or show-hidden.
- A visited-folder mutation then restart (the dest was written on disk, not
  by the debounce writer).
- GUI paint of Grid/Details after hydrate (no capture dest).

`loginctl show-seat seat0`: `CanGraphical=yes`, empty `ActiveSession` /
`Sessions`. No grim / Sunshine / Moonlight. SSH cannot click Construct.

## Leftover

Still open. Closing the production leftover needs operator Files use (or a
DRM/input harness) on a **current-revision** seat so a real view/sort/hidden
change is what survives restart. The RPM unit still omits
`XDG_CONFIG_HOME`; Seat 15 carries a local drop-in dest. Dell and Surface
were not dest'd. Do not flip `production_admitted`.
