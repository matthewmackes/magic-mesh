# Transfers — the universal file-operations surface (design)

*Operator survey 2026-07-05 (20-Q `/plan`, session 759c1f91). The one place every
byte that moves is born, tracked, and completed: sftp · rsync · wget/HTTP · browser
downloads + scrapes · node↔node · uploads/downloads to the mesh music library.*

## Identity

A **new "Transfers" tab inside the File Browser** (`mde-files-egui`) fronted by a
daemon-owned queue. Execution lives in a **mackesd `transfers` worker** (§9 doctrine
— the GUI renders, the daemon owns lifecycle); jobs therefore survive shell restarts,
run on headless nodes, and any node can host them. This is the download manager, the
node-mover, and the sync engine folded into one honest ledger.

## Locks (the 20)

| # | Decision | Lock |
|---|---|---|
| Q1 | Placement | A tab in the **File Browser** + the dock's Files cell shows an **active-transfers badge** |
| Q2 | Name | **"Transfers"** (display-tier title on the shared bar) |
| Q3 | Execution home | **mackesd `transfers` worker** — daemon-side queue + lifecycle; GUI is a renderer |
| Q4 | Job model | **Per-protocol lanes** (an sftp lane, rsync lane, http/wget lane, browser-download lane, node lane) — each keeps its tool's native semantics; a shared `TransferJob` envelope carries id/source/dest/method/policy/state |
| Q5 | Endpoints | **Mesh peers + any foreign sftp/rsync host + any wget URL** — the universal reach |
| Q6 | Node↔node transport | **Stage via the Syncthing share** (`/mnt/mesh-storage`) — a node→node move writes into the mesh-share and the substrate replicates; survives both peers rebooting. (Foreign-host moves go direct via Q7.) |
| Q7 | Engines | **System binaries, bounded** — shell out to real `rsync`/`sftp`/`wget` via the bounded-proc path; authentic semantics (rsync deltas, wget resume); progress parsed from their output |
| Q8 | Browser tie | **One queue — the browser enqueues here.** Transfers IS the download manager (BROWSER-DD-10's manager + DD-12's download/capture events fold in) |
| Q9 | Music destination | The **mesh music library dir** — auto-register the Navidrome library path (the `mcnf-mesh-media` rclone mount / the `/mnt/mesh-storage` music dir) as a standing **"Music Library"** destination; a track dropped there lands in the shared library + replicates |
| Q10 | Destination registry | **Auto only** — peers from the roster, Music + mesh-share from node state; arbitrary sftp/URL targets are **entered per-job, not saved as pins** |
| Q11 | History + resume | **Full persistent ledger + resume** — every job persists in the worker's store; interrupted transfers resume where the tool allows (`rsync --partial`, `wget -c`); history survives reboots + is queryable |
| Q12 | Throughput | **Parallel + per-job throttle** — N concurrent (configurable cap) with an optional per-job bandwidth limit (`rsync --bwlimit` / `wget --limit-rate`) |
| Q13 | New-transfer entry | **New Transfer dialog + drag-drop onto a destination + right-click "Send to →"** in the Files browser — three entry points, one queue |
| Q14 | Bus verb | **One `transfer.submit(job)` + lifecycle** (`transfer.cancel/pause/resume/list`) — a small typed verb set both GUI and CLI drive (§9 CLI parity); the worker routes the job to its lane |
| Q15 | Integrity | **Optional per-job** — a "verify integrity" toggle (size + checksum on completion, visible verified✓/mismatch✗); off by default, a mismatch is a failure not silent |
| Q16 | Menu spine | **Shared with the Files bar** — the Transfers tab reuses the File Browser's MenuBar with transfer items appended (not a separate spine); MENUBAR-SWEEP-aligned |
| Q17 | Scrapes | **Browser scrapes, hands files to Transfers** — the Power-mode scraper crawls/extracts; each output file/export is submitted here for the destination + ledger. Transfers moves outputs, doesn't crawl |
| Q18 | Notify | **Completion/failure → the chat feed** — a job finishing/failing emits a signed message on the `event/notify/*` lane the chat worker folds (CHAT-FIX-2); no new notification surface; the tab shows live progress |
| Q19 | Scheduling | **Recurring sync pairs IN v1** — a saved (schedule + source/dest) rsync-mirror pair driven by a mackesd timer, alongside one-shot jobs |
| Q20 | MVP slice | **Whole surface in one epic** — all lanes (sftp/rsync/wget/browser/node/music) land together; no partial Transfers tab ships |

## Architecture

```
  mde-files-egui  ── Transfers tab (renderer)
        │  transfer.submit / .cancel / .pause / .resume / .list  (typed Bus verbs, §9)
        ▼
  mackesd `transfers` worker  (rank per BUG-STORAGE-1 lesson)
        │  TransferJob envelope → route by method to a lane
        ├── http/wget lane      → wget -c --limit-rate           (+ browser-enqueued downloads/scrapes, Q8/Q17)
        ├── sftp lane           → sftp/ssh to a foreign host
        ├── rsync lane          → rsync --partial --bwlimit      (+ recurring sync pairs, Q19)
        ├── node lane           → write into /mnt/mesh-storage → Syncthing replicates (Q6)
        └── music lane          → the Navidrome library dir destination (Q9)
        │  persistent ledger (queued/running/done/failed) + resume state (Q11)
        │  completion/failure → event/notify/* (chat, Q18)
        ▼
  bounded-proc path (rsync/sftp/wget) — system binaries, progress parsed (Q7)
```

- **Destinations** are a live list: peers auto-fill from the roster, **Music Library**
  + mesh-share auto-register from node state (Q9/Q10); foreign hosts/URLs are typed
  per-job. Drag-drop + right-click "Send to →" target this list (Q13).
- **One `TransferJob` envelope, per-lane executors** (Q4/Q14): the submit verb is
  singular; the method field routes to the lane; each lane keeps its tool's semantics.
- **CLI parity** (§9): the same verbs drive a `mackesd transfer …` CLI.

## Acceptance (each runtime-observable)

- A wget/HTTP download, an sftp put/get to a foreign host, an rsync mirror, a
  node→node move (via the mesh-share), and a drop into the Music Library each complete
  end-to-end and appear in the ledger.
- Interrupting a running http/rsync job and resuming continues (not restarts).
- N jobs run concurrently up to the cap; a per-job bandwidth limit is honored.
- The browser hands a download AND a scrape output to the queue; both land at their
  destination with a ledger entry.
- A completed and a failed job each surface as a signed chat message.
- A recurring sync pair fires on its schedule and mirrors.
- The Transfers tab renders progress live; the dock Files cell badges active count.

## Risks

- **Progress parsing** off `rsync`/`wget` stdout is version-fragile — pin the parse to
  the shipped tool versions + fall back to a coarse running/done state, never a fake %.
- **Bounded-proc discipline**: every shelled tool runs under the existing bounded path
  (no unbounded child procs), and credentials for foreign sftp/rsync never hit the
  journal (the Eagle sudo-in-journal hygiene lesson).
- **Music-dir writes** must land in the replicated library path, not a per-instance
  local dir (the MEDIA-6 flat-file lesson) — verify the destination is the shared mount.
- **Q20 "one epic"** means the surface is held `[>]` until every lane works — a longer
  time-to-first-green than a vertical slice; mitigated by building lanes in parallel
  across the farm.

## Out of scope (v1)

- A generic cloud-storage browser (S3/Spaces beyond the Music bucket) — the Music
  destination is the only object-store target.
- Torrent / p2p-swarm transports.
- Per-service transfer ACLs (flat-trust mesh, §8).
