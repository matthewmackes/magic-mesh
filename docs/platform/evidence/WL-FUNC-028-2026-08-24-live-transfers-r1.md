# WL-FUNC-028 leftover — Seat 15 live Construct Transfers probe — r1

Date: 2026-08-24  
Classification: leftover-honesty / live-seat CLI+store+Bus+capture gap;
**not** live GUI paint/click, **not** a pair add, **not** dest-cut, **not**
`production_admitted`  
Source revision (control tree): `cef15eabf033f40352afaab5fef9ebff2c665b0c`  
Installed identity: `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
Seat: Seat 15 `172.20.0.15` (`Basement-Test-Workstation`)  
Observed: `2026-08-24T16:24:23Z`–`2026-08-24T16:25:43Z`  
`production_admitted: false`

Read-only SSH from the control host (`mm@`,
`/root/.ssh/mackes_mesh_ed25519`). `sudo -n` used only to list store, Bus
files, journal, and DRM holders. No `sync-pair add`, no inbox mkdir, no
enroll, no package install, no `systemctl` mutate, no Sunshine start, no
`seat-remote-input` / uinput injection, no invented dest.

Prior Dell leftover-028 / dest-cut gap:
`WL-FUNC-028-2026-08-23-live-construct-transfers-gap-r1.md`. In-tree editor
CLI parity (not installed):
`WL-FUNC-028-2026-08-23-transfers-editor-cli-parity-r1.md`.

## Observed (Seat 15)

| Field | Value |
|---|---|
| hostname | `Basement-Test-Workstation` |
| RPM | `magic-mesh-13.0.0-35.x86_64` |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| `/usr/bin/mde-shell-egui` | 60819776 bytes, mtime `2026-08-22 21:56:45` (local) |
| sha256 (shell) | `faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14` |
| sha256 (mackesd) | `b27a035167946e182d65f7784384b1b54c2418e63a515cb28fcb7127cbeb1009` |
| Construct | pid `476582` as `root`, `mde-shell-egui.service` active since `2026-08-24 11:50:07 EDT` |
| DRM | pid `476582` holds `/dev/dri/card1` (five fds) |

## Transfers CLI / store

`mackesd transfer` lists `sync-pair`. Subcommands present: `add`, `remove`,
`list`. Operator `list` printed `no sync pairs saved`. `--json` printed
`[]`.

Store on disk:

| Path | State |
|---|---|
| `/var/lib/mde/transfers` | `root:root` `0755`; not writable by `mm` |
| `/var/lib/mde/transfers/sync-pairs` | empty directory (mtime `2026-08-23 09:27`) |
| `/var/lib/mde/transfers/inbox` | **absent** |
| `/var/lib/mde/transfers/ledger` | empty directory |

Dell leftover-028 (`/tmp/mcnf-sync-src` → `/tmp/mcnf-sync-dst`) is **not**
on this seat. This probe did not add a pair and did not create the inbox.
`mackesd transfer destinations` listed the existing auto dest `mesh-share`
(`node /mnt/mesh-storage`); that row was not used.

dest-cut `mackesd` packed `save-sync-pair` / `remove-sync-pair` /
`invalid sync pair id` / `no sync pair`. Function name
`ensure_operator_inbox` is not in the binary (stripped). The live worker
logged `transfers worker up` at `2026-08-24T16:00:10Z` against
`store=/var/lib/mde/transfers` and still left `inbox` absent.

## Dest-cut Construct editor (packed, not painted)

Byte counts in `/usr/bin/mde-shell-egui` (not a live frame):

| Literal | Count |
|---|---|
| `No sync pairs saved.` | 1 |
| `Create one here or with` | 1 |
| `next-run and last-result come from the transfers worker` | (adjacent to empty-store copy) |
| `save-sync-pair` | 1 |
| `remove-sync-pair` | 1 |
| `New transfer` | 3 |
| `Open Transfers` | 1 |
| `malformed pair id` | 1 (adjacent to `New transfer (Ctrl+N)`) |
| `no sync pair` | 0 |
| `Queued; next tick` | 0 |
| `recurring sync pairs` | 1 |

The dest-cut editor is therefore packed in this Construct, and it still
carries dest-cut refuse copy `malformed pair id`. CLI-parity copy from
in-tree `43459f809` is not installed.

## Worker notify

Transfers worker heartbeat topic
`state/mackesd/data/workers/transfers` is live. Latest snapshot at
`2026-08-24T16:25:43Z`:

| Field | Value |
|---|---|
| ulid | `01M0T9EYE9MG0DEENQ0FMME4PE` |
| worker_id | `transfers` |
| state | `running` |
| generation | `307` |
| node_id | `peer:Basement-Test-Workstation` |
| state_since_ms | `1787587210571` |
| pair ids in body | none |

`state/collab/transfer-jobs` retained body is `{"jobs":[]}`.

`event/notify/transfers` does **not** exist as a persist dir.
`mde-bus history event/notify/transfers --bus-root /run/mde-bus` printed
nothing (`HISTORY_RC=0`). SQLite `messages` count for that topic is **0**.
Active notify topics on this seat are `event/notify/peer` and
`event/notify/updates` only. There was no leftover-028 (or any other)
rsync `done` notify here — unlike Dell.

## GUI / capture dest

`loginctl` session `1` is class `manager`, `Type=unspecified`, empty
`Seat` / `Display`. seat0 `CanGraphical=yes` with empty `ActiveSession` /
`Sessions`. That is the MCNF DRM-service model (Construct holds card1),
not an absent GUI.

Physical input present: Keychron K6 (`event4`/`event5`) and PixArt Dell
MS116 (`event3`). Dock journal from pid `2353` (before today's shell
restart) last opened Maps & Location at `2026-08-24T11:24:27Z`; Mesh Teams
(`Communications`) was opened from the dock, not Transfers mode. Pid
`476582` has **no** `nav_bar` / `open_surface` / `OpenTransfers` journal
lines. Seven-day journal has no `OpenTransfers` / `save-sync-pair` /
`leftover-028` GUI lines.

Capture dests:

| Path | State |
|---|---|
| `/usr/bin/sunshine` | present; **no** `sunshine.service` unit file; no process |
| grim | absent |
| Moonlight | absent |
| `/usr/bin/ffmpeg` | present |
| `/usr/libexec/mackesd/seat-remote-input` | present; not invoked |
| `/dev/uinput` | present (`root:input` `660`) |

Starting Sunshine or injecting Ctrl+J / a click would invent a dest and
still could not record the frame. Root Construct fds are not readable as
`mm`.

## Non-claims

- Construct Transfers was not opened, saved, or removed on Seat 15.
- No sync-pair was added. leftover-028 was not copied from Dell.
- Inbox was not created. `production_admitted` was not flipped.
- No dest was invented or signed.

## Leftover / blocker

Seat 15 has the dest-cut CLI producer, an empty store, a running
transfers worker with **no** pair-fire notify, and a packed Transfers
editor on a used DRM Construct. That is not live paint/click of
Communications Transfers, and it is not CLI-parity GUI.

Closing S2 still needs live Construct Transfers paint/click on a
current-revision seat (dest that includes `43459f809`, or dest-cut plus
a graphical seat/capture dest **and** an operator-visible pair). Fixtures,
Dell leftover-028, and this empty Seat 15 store do not close it.
