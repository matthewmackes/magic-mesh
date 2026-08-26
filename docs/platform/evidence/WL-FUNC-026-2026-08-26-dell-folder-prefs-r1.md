# WL-FUNC-026 leftover honesty — Dell folder-prefs persist — r1

Date: 2026-08-26  
Observed: `2026-08-26T11:57:00Z`–`2026-08-26T12:02:00Z`  
Classification: leftover-honesty / live-seat observe; **not** operator
FileBrowser view/sort/hidden mutation, **not** dest hydrate, **not**
`production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at `b6fd8aeabcb850a11396a4a412b2ddd5c79b21ce`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c`  
Control host: `rocky9-kvm2` (no Nebula overlay; SSH underlay)  
Seat: Dell `DELL-LAPTOP` `172.20.146.225` overlay `10.42.0.4`  
Live topology mesh-id (governed, not minted here): `mcnf-clean-20260728`  
`production_admitted: false`

Read-only SSH as `mm@` with `/root/.ssh/mackes_mesh_ed25519` and `sudo -n`.
No `seat-update-warning`. No dest JSON. No Construct restart (seat0
`Sessions=` empty; Construct holds DRM; restart would drop the login
curtain without power-honor). No Sunshine. No uinput.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-026`.
- Leftover: operator Files view/sort/hidden then restart.
- Prior Surface dest-cut observe (not Dell, not operator GUI):
  `WL-FUNC-026-2026-08-25-surface-folder-prefs-r1.md`.
- Prior Seat 15 dest hydrate (not operator use):
  `WL-FUNC-026-2026-08-24-seat15-folder-prefs-r1.md`.
- Packaged unit pin:
  `WL-FUNC-026-2026-08-24-xdg-config-home-unit-r1.md`.

## Path used

`mde-files-egui` `default_config_file("files-folder-prefs.json")` is
`$XDG_CONFIG_HOME/mcnf/files-folder-prefs.json`, else
`$HOME/.config/mcnf/files-folder-prefs.json` (`None` if both unset).

Dell dest-cut `bc14a22d7` ships the pin in the packaged unit (not a
Seat-15-style drop-in):

| Item | Value |
|---|---|
| Seat | `DELL-LAPTOP` underlay `172.20.146.225` overlay `10.42.0.4` |
| SSH | `mm@` / `sudo -n` / `/root/.ssh/mackes_mesh_ed25519` |
| RPM | `magic-mesh-13.0.0-35.x86_64` (buildtime Tue 25 Aug 2026 11:33:54 AM EDT) |
| `mackesd --version` | `13.0.0 "Construct" · bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c · 2026-08-25 · dev` |
| Unit | `/usr/lib/systemd/system/mde-shell-egui.service` |
| Drop-ins | cloud-arm / music-action / browser-vm-rdp / resource-publisher HMAC only — **no** `60-xdg-config.conf` |
| Live environ | `XDG_CONFIG_HOME=/root/.config`; `HOME` unset; `USER=root` |
| Expected store | `/root/.config/mcnf/files-folder-prefs.json` |
| Observed | `/root/.config/mcnf` **absent**; store **absent** |

`/root/.config` exists (`btop`, `glib-2.0`, `gtk-3.0`, `procps`,
`pulse`, `sublime-music` only). Packed shell contains ASCII
`files-folder-prefs.json` (count 1). Live pid `204172` environ includes
the unit pin. Resolver can write after a FileBrowser mutation; nothing
has written yet.

Construct pid `204172` as `root` has been up since `2026-08-25 15:46:06 EDT`
(`NRestarts=0`) after the dest-cut install. DRM: eDP-1 connected/enabled;
pid holds `/dev/dri/card1`. Overlay ping to LH1 `10.42.0.1` 16.3 ms.

## GUI mutation vs dest hydrate

Dest JSON was **not** planted. No FileBrowser view/sort/hidden click
(no Sunshine, no uinput). Prefer persist-file write after GUI mutation
over a destructive Construct restart; the store was already absent, so
a restart would not have proven a write.

Dock opens on this pid (journal):

```text
2026-08-26T11:16:42.151547Z  open_surface  Maps & Location
2026-08-26T11:17:23.027124Z  open_surface  Mesh Teams
```

Touchpad libinput events on the same morning show a live operator seat.
Production `Surface::Files` is a retired top-level route: dest-cut
`bc14a22d7` (and this worktree) alias it into Communications
(`communications.open_files()` → `MeshTeamsApp::Files` /
`Mode::Files`). `files_panel` is not mounted in the production body
(`Surface::Files => unreachable!(...)`). Mesh Teams Files mode is
space-linked references (`mde-collab-egui` `files_body`), not
`FileBrowser` view/sort/hidden. `FileBrowser` is constructed
(`real_browser()`) and `pump_transfers` runs every frame; folder-prefs
flush is `files_panel` → `pump_ops` → `flush_persisted_if_due`, so a
Mesh Teams Files visit cannot create `files-folder-prefs.json`.

`loginctl show-seat seat0`: `CanGraphical=yes`, empty `ActiveSession` /
`Sessions`. Construct owns the DRM seat. Restart skipped.

## Farm

Focused gate `cargo test -p mde-files-egui` on `.90` slot 2 (host pin
per leftover unit; `.50`/`.90`/`.170`). Cold compile of
`mde-files-egui` **failed** (`exit 101`, `2026-08-26T12:02:46Z`–
`2026-08-26T12:11:14Z`):

```text
error[E0609]: no field `bookmarks` on type `std::vec::Vec<bookmarks::UserBookmark>`
 --> crates/desktop/mde-files-egui/src/bookmarks.rs:232:33
```

That path is outside this unit's write scope (`model/mod.rs` only).
A warm `.130` result `a1183c2d3475` at this dirty HEAD reported
`210 passed` in 0.75s without rebuilding the lib; it is not a cold
compile of the tree this unit rsynced.

## What this does not prove

- Operator FileBrowser toolbar/menu changed view, sort, or show-hidden.
- A visited-folder mutation then restart.
- GUI paint of Grid/Details after hydrate.
- Debounce writer creating the store.

## Leftover

Still open. Persist path is observed and writable in principle
(`/root/.config/mcnf/files-folder-prefs.json` via the packaged
`XDG_CONFIG_HOME` pin). GUI mutation did **not** write
`files-folder-prefs.json`. Closing the production leftover needs a
mounted FileBrowser GUI view/sort/hidden change on a current-revision
seat so a real mutation is what survives restart. Dest hydrate is not
operator use. Do not invent a dest. Do not flip `production_admitted`.
Do not restart Construct without power-honor while it holds seat0 DRM.
