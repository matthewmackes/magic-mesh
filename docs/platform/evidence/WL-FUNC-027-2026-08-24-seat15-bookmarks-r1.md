# WL-FUNC-027 leftover — Seat 15 bookmarks persist dest — r1

Date: 2026-08-24  
Observed: `2026-08-24T15:47:28Z`–`2026-08-24T15:50:14Z`  
Classification: leftover dest / live restart hydrate; **not** operator GUI
pin/rename/reorder/remove/navigate, **not** `production_admitted`, **not** a
package install  
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

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-027`.
- Leftover: live restart/navigate evidence (fixtures do not satisfy
  production).
- Prior absence: `WL-FUNC-025-2026-08-23-live-seat-files-archive-r1.md`,
  `WL-FUNC-028-2026-08-23-installed-sync-pair-cli-r1.md`.

## Path used

`mde-files-egui` `default_config_file("files-bookmarks.json")` is
`$XDG_CONFIG_HOME/mcnf/files-bookmarks.json`, else
`$HOME/.config/mcnf/files-bookmarks.json` (`None` if both unset).

The root DRM unit did not set `HOME` or `XDG_CONFIG_HOME`. Live pid `2353`
therefore had store path `None`. Same Seat 15 drop-in as WL-FUNC-026
(`XDG_CONFIG_HOME=/root/.config`) is what lets hydrate run.

| Item | Value |
|---|---|
| Drop-in | `/etc/systemd/system/mde-shell-egui.service.d/60-xdg-config.conf` |
| Store | `/root/.config/mcnf/files-bookmarks.json` |
| Mode | `0600` regular file, `nlink=1`, not a symlink |
| Size | 180 bytes (cap 64 KiB / 48 bookmarks) |
| SHA-256 | `2794853d22fa5ae7f93ef3e2dfa616c5d64b25572a25a4317642f0dcd4b44dee` |

Bounded dest body (two valid pins: absolute `/home/mm` labeled
`WL-FUNC-027 dest pin`, and `local:downloads`). No `peer:` route, no `.` /
`..` segments, labels under 128 chars.

## Restart

Identical SHA and inode across three `mde-shell-egui` restarts (pids
`2353` → `472289` → `474600` → `476582`). A `surfaces`-completed process
Drop did not rewrite the store.

## Hydrate (observable)

Pid `476582` constructed `FileBrowser` at `surfaces`
`2026-08-24T15:50:13Z`. inotify:

```text
OPEN ACCESS ACCESS CLOSE_NOWRITE  /root/.config/mcnf/files-bookmarks.json
```

No `MODIFY` / `CLOSE_WRITE`. SHA after hydrate matched SHA before restart.
Unit remained `active`.

## What this does not prove

- Operator pin / rename / reorder / remove in Places.
- Navigate-on-activation of a hydrated pin (no input/capture dest).
- GUI paint of the user Places section.

`loginctl show-seat seat0`: `CanGraphical=yes`, empty `ActiveSession` /
`Sessions`. No grim / Sunshine / Moonlight.

## Leftover

Still open. Closing the production leftover needs operator Files pin then
restart and activate on a **current-revision** seat. The RPM unit still
omits `XDG_CONFIG_HOME`; Seat 15 carries a local drop-in dest. Dell and
Surface were not dest'd. Do not flip `production_admitted`.
