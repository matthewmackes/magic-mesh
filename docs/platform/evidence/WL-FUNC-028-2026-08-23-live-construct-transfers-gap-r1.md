# WL-FUNC-028 leftover — live Construct Transfers honesty vs 13.0.0-35 — r1

Date: 2026-08-24  
Classification: leftover-honesty / live-seat GUI+Bus gap; **not**
`production_admitted`, **not** dest-cut, **not** a package install  
Source revision: `f5362d86545a6444dc3871fbaf42d3aae027a398`  
Control host: `rocky9-kvm2`  
Seat: Dell `172.20.146.225` (`DELL-LAPTOP`)  
Observed: `2026-08-24T00:03:11Z`  
`production_admitted: false`

Read-only SSH from the control host. No `sync-pair add`, no enroll, no
package install, no `systemctl` mutate, no invented dest.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-028` leftover
  (live Construct Transfers).
- Prior CLI pair: `WL-FUNC-028-2026-08-23-live-sync-pair-next-run-r1.md`.
- In-tree editor CLI parity (not on this dest-cut):
  `WL-FUNC-028-2026-08-23-transfers-editor-cli-parity-r1.md`.

## Observed on dest-cut `13.0.0-35`

| Fact | Value |
|---|---|
| RPM | `magic-mesh-13.0.0-35.x86_64` |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| Construct | `/usr/bin/mde-shell-egui` pid `2055123` as `root`, elapsed ~21h |
| Pair `leftover-028` | store present; `last_result=done`; `peer_reachable=true`; `next_run_ms=1787531249862` |
| Inbox | `/var/lib/mde/transfers/inbox` mode `1777`, empty |
| Dest-cut binary | ships Transfers editor + inbox drain (`Sync pairs`, `New transfer`, `save-sync-pair`, `No sync pairs saved.`) |

`leftover-028` dests were not invented here. They remain the prior CLI
pair `/tmp/mcnf-sync-src` → `/tmp/mcnf-sync-dst`.

## Live Bus (worker, not GUI)

`event/notify/transfers` `01M0RF6RH1HGNES86CRMTZ7R3B` at
`ts_unix_ms=1787527651873` is the leftover-028 fire:

- `summary`: `transfer 17875276 completed (rsync)`
- `transfer_id`: `1787527649862-000000000003-2a9612ad`
- `transfer_state`: `done`
- `method`: `rsync`
- `host`: `DELL-LAPTOP`

That id matches store `last_fired_ms=1787527649862`. The transfers
worker heartbeat topic (`state/mackesd/data/workers/transfers`) does
not name pair ids. Dest-cut Construct folds
`/var/lib/mde/transfers/sync-pairs/*.json`; it does not publish
`SaveSyncPair` / `RemoveSyncPair` onto Bus. GUI save/remove write the
inbox only.

## Honest gap vs dest-cut `13.0.0-35`

1. **No live GUI paint/click.** `loginctl` session `1` is class
   `manager`, `Type=unspecified`, empty `Seat` and `Display`. seat0
   reports `CanGraphical=yes` but empty `ActiveSession` / `Sessions`.
   No Sunshine / Moonlight / grim dest. Root Construct fds are not
   readable as `mm`. Injecting Ctrl+J or a click would invent an input
   dest and still could not record the frame.
2. **Dest-cut editor is not CLI-parity.** dest-cut `save_sync_pair_draft`
   uses the mutable draft id (a rename mints a second row), refuses
   Save when the projection vanished, and `close()` drops the notice so
   a successful save never shows the CLI queued-next-tick line.
   Refusal copy is `malformed pair id` / `unknown pair id`, not the
   CLI `invalid sync pair id` / `no sync pair … in the store` text.
   In-tree `43459f809` already matches the CLI. That fold is after
   dest-cut `7e3474eeb` and is not installed.
3. **No `transfers.rs` change on this leftover.** The in-tree editor
   already matches the CLI producer. Putting that editor on Dell would
   be a new dest / RPM, which this unit must not invent.

## Non-claims

- Construct Transfers was not opened, saved, or removed on the seat.
- `production_admitted` was not flipped.
- No dest was invented or signed.

## Blocker

Live Construct Transfers paint/click on a current-revision seat (dest
that includes `43459f809`, or dest-cut plus a graphical seat/capture
dest). Fixtures and the CLI leftover-028 pair do not close S2.
