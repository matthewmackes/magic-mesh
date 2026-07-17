# File manager — fully functional + automatic sshfs mesh access (FILEMGR-1..12)

> **Status: LOCKED 2026-07-02** — 25-question operator survey (3 rounds: core ops /
> sshfs mesh / UX-integration). Make the `Surface::Files` file manager fully
> functional (the full POSIX operation set) and give it **automatic access over
> sshfs to every mesh node**. Today `mde-files-egui` + `mde-files` do navigation +
> Send-To with a `demo_data.rs` mockup and one real op (`copy_ask`); this epic
> builds the real thing.

## The locks

### Round 1 — core operations

| # | Fork | Lock |
|---|------|------|
| 1 | Op scope | **Full POSIX + archives** — copy/move/rename/delete/mkdir/new-file/symlink + chmod/chown/timestamps, properties, recursive search (name+content), archive create/extract, duplicate/hardlink. |
| 2 | Execution | **Real ops in the `mde-files` backend behind an injectable `FileOps` trait** (std::fs + nix); the egui surface stays render+request; tests use a fake FS; **`demo_data.rs` deleted** (§7 no mockups); §9 typed API, no raw shell. |
| 3 | Delete | **Permanent delete + a confirm dialog** (NO trash). The confirm is the safeguard. |
| 4 | Conflicts | **Per-item Overwrite / Skip / Keep-both(auto-rename) + apply-to-all**; directories **merge recursively** (children re-run the conflict logic). |
| 5 | Progress | **Async op on a worker thread + a background queue** — live progress (files/bytes/ETA), pause/resume (transfers), cancel; UI never blocks; a cancel reports what actually completed (no half-files claimed done). |
| 6 | Undo | **No undo.** Operations are final; the confirm dialogs are the safeguard (consistent with permanent-delete). |
| 7 | Selection | **Full desktop selection** — click / Ctrl-click / Shift-range / Ctrl-A / rubber-band; every op applies to the whole set as one queued op; keyboard-navigable. |
| 8 | Permissions | **Properties dialog** — owner/group/other × rwx grid + live octal + direct octal entry + owner/group fields; chmod as the user; **chown offered only when actually permitted** (root/CAP_CHOWN), honestly disabled otherwise (§7); recursive-apply for dirs. |
| 9 | Search | **Recursive name + full-text content search**, async streaming results (cancellable), filters (type/size/mtime); results are a normal file view; works on local + mounted mesh paths (honest "remote is slower"). |
| 10 | Archives | **Create + extract zip / tar(.gz/.xz/.zst)** via Rust crates (zip/tar/flate2/xz2/zstd — no shell-out, §9); double-click browses an archive in-place (extract-here / extract-to); progress via the async queue. Password-zip out of scope v1. |

### Round 2 — sshfs mesh access

| # | Fork | Lock |
|---|------|------|
| 11 | Mount model | **On-demand auto-mount per peer at a stable path** (`/run/user/<uid>/mde-mesh/<host>`); a "Mesh" sidebar root lists peers; navigating in lazily mounts over the Nebula overlay; **idle mounts auto-unmount**. |
| 12 | Node source | **The live mesh roster** (`mackes_mesh_types::peers` / the Mesh-DNS `<host>.<mesh>` zone) — every enrolled peer with online/reachable pips; offline peers list greyed (honest, can't mount); self skipped; resolves by `<host>.<mesh>` over the overlay. |
| 13 | Auth | **A shared mesh SSH keypair, node-sealed** in the secret store; sshfs uses it over the overlay. (Accepted blast-radius tradeoff vs CA-certs; revoke = re-key.) |
| 14 | Remote root | **Home by default** (mount the mesh user's `~`); **an explicit GUI action escalates to full-filesystem** (`/`) access. Least-privilege default + full-node reach on demand. |
| 15 | Offline/drops | **Honest unreachable state + auto-reconnect with backoff**; a bounded connect timeout (never a frozen UI); **frozen/stale sshfs mounts detected + recovered** ("connection lost — reconnecting"); in-flight ops on a dropped mount fail cleanly + typed. |
| 16 | Cross-node copy | **Direct peer-to-peer transfer** for A→B (reuse Send-To / a peer-side helper — no double-hop through the browsing node); local↔node is a straight sshfs read/write; **sshfs-relay fallback** if no direct path; one queued transfer with real progress. |
| 17 | Mount owner | **A `mackesd` mesh-mount worker** owns the lifecycle (holds the sealed key + roster; mounts on `action/mesh-mount/<host>`; publishes `state/mesh-mount/*`; idle-unmount + reconnect + frozen-mount recovery). The Files surface requests a mount + browses the returned path. §6-clean. *(operator did not answer — took the §6-clean recommended default.)* |
| 18 | Perf | **Tuned sshfs** (attr/dir cache, `kernel_cache`, `big_writes`/large `max_read`, compression for WAN) + **async listings** (per-pane spinner, never blocks) + lazy on-demand thumbnails/previews for remote paths + a "remote" badge; manual refresh busts the cache. |
| 19 | Remote safety | **Extra typed-arming for destructive ops on a REMOTE mount AND any op under an escalated full-fs/root mount** — the confirm names the target node + path (type the hostname). Mirrors the storage-plane arming discipline. |

### Round 3 — UX + integration

| # | Fork | Lock |
|---|------|------|
| 20 | Views | **List + Icons/Grid(thumbnails) + Details(columns)**, click-header sort by name/size/type/mtime asc/desc, dirs-first toggle, show-hidden (Ctrl+H); view+sort **persist per-folder**. |
| 21 | Navigation | **Breadcrumbs + editable path box + back/forward/up history + TABS + DUAL-PANE + a sidebar** (Places + pinnable bookmarks + the Mesh node tree). Dual-pane makes cross-node work fluid. |
| 22 | Preview | **Thumbnails for images/video** (lazy off-thread, cached, size-capped, freedesktop spec) + a **preview pane** (image render, text/code highlighting, media metadata); spacebar quick-look; remote previews lazy/on-demand. |
| 23 | Open | **Built-in shell viewers only** (image/text/media) — no external app spawn (fits the bare-DRM self-contained shell); unknown types → honest "no handler". |
| 24 | Drag-drop | **Full DnD** — move (default) / copy (Ctrl) within a pane, between dual-panes (incl. local↔node, node→node via the Q16 direct path), and **onto sidebar bookmarks / Mesh nodes** (= transfer there); drop feedback shows target + action. |
| 25 | Integration | **Stays `Surface::Files`** (mde-files-egui over mde-files, no separate window); right-click **Send-To** a peer (reuse), **Send in Chat** (reuse the NOTIFY-CHAT file message-kind), and **cut/copy/paste share the shell clipboard** (paths, cross-surface). |

## Architecture

```
Surface::Files (mde-files-egui)                       mackesd (mesh tier)
┌───────────────────────────────────────┐            ┌──────────────────────────────┐
│ sidebar: Places · Bookmarks · Mesh▾    │ action/    │ mesh_mount worker            │
│   Mesh: <host>.<mesh> + presence pip   │ mesh-mount │  · sealed shared SSH key     │
│ tabs · dual-pane · breadcrumbs         │──────────▶ │  · roster-driven peer list   │
│ views: list/grid/details · DnD · sel   │◀────────── │  · sshfs mount (home|/, tuned)│
│ op queue panel (progress/pause/cancel) │ state/     │  · idle-unmount · reconnect  │
│ dialogs: conflict · properties · arm   │ mesh-mount │  · frozen-mount recovery     │
│ preview pane + thumbnails (built-in)   │            │  publishes mounted-path      │
└──────────────┬────────────────────────┘            └──────────────────────────────┘
               │ FileOps trait (injectable)
   mde-files backend (lib)
   · POSIX ops (std::fs + nix): copy/move/rename/delete/mkdir/symlink/chmod/chown
   · conflict engine · async op queue (progress/cancel) · archives (zip/tar crates)
   · recursive name+content search · direct A→B transfer (reuse send_to)
   · demo_data.rs DELETED
```

- **`mde-files` (backend, lib)** owns every operation behind the `FileOps` trait
  (real = std::fs+nix; fake for tests) — copy/move/rename/delete/mkdir/symlink/perms,
  the conflict engine, the async op queue, archives, search, the direct-transfer glue.
  Mounted mesh paths are just local paths to it (the worker did the mount).
- **`mde-files-egui` (Surface::Files)** is render + request: views, navigation, selection,
  DnD, the op-queue panel, the dialogs (conflict/properties/typed-arming), preview/thumbnails,
  the Mesh sidebar tree. §4 Quazar tokens.
- **`mackesd` mesh_mount worker** owns the sshfs lifecycle over the overlay (sealed key,
  roster, mount/unmount/health/reconnect, home-vs-root escalation). §6: the mesh/overlay/key
  concerns stay mesh-side; the desktop surface only requests + browses.
- **Reuse**: Send-To (cross-node transfer), the NOTIFY-CHAT file message-kind (Send in Chat),
  the roster + Mesh-DNS (peer list), the secret store (sealed key), the shell clipboard.

## The units (FILEMGR-1..12)

- **FILEMGR-1 — the `FileOps` backend core.** The injectable trait + real (std::fs+nix)
  impl: copy/move/rename/delete/mkdir/new-file/symlink/duplicate/hardlink/chmod/chown/
  timestamps; **delete `demo_data.rs`**; fake-FS unit tests. §9 typed, no shell.
- **FILEMGR-2 — the async op queue + conflict engine.** Worker-thread ops with live
  progress/pause/cancel + a queue; the per-item Overwrite/Skip/Keep-both conflict resolver
  with apply-to-all + recursive dir-merge; cancel reports true completion.
- **FILEMGR-3 — archives.** zip / tar(.gz/.xz/.zst) create + extract + browse-in-place via
  Rust crates; queued progress.
- **FILEMGR-4 — recursive search.** Async streaming name + full-text content search with
  filters; results as a file view; cancellable; works on mounted mesh paths.
- **FILEMGR-5 — the mackesd `mesh_mount` worker.** sshfs lifecycle over the overlay:
  mount on `action/mesh-mount/<host>` (home default; escalate verb → `/`), tuned mount opts,
  publish `state/mesh-mount/*`, idle-unmount, reconnect+backoff, frozen-mount detection+recovery.
- **FILEMGR-6 — the shared mesh SSH key + sshd overlay bind.** Provision/seal the shared
  keypair to each node (secret store), install it for the mesh user, ensure sshd accepts it
  over the overlay only; re-key path for revocation.
- **FILEMGR-7 — direct peer-to-peer transfer.** A→B routed directly (reuse Send-To / a
  peer-side helper), sshfs-relay fallback, one queued transfer with progress; wired into
  copy/move + DnD-onto-node.
- **FILEMGR-8 — the Files surface shell.** Views (list/grid/details + sort + per-folder
  memory), navigation (breadcrumbs/path-box/history/**tabs**/**dual-pane**/sidebar), selection
  (click/Ctrl/Shift/Ctrl-A/rubber-band), full DnD. §4 tokens.
- **FILEMGR-9 — the Mesh sidebar tree.** Roster-driven peer list + presence/reachability pips;
  navigate-to-mount (request FILEMGR-5); the **home↔full-fs escalation** GUI action; honest
  offline/unreachable rows.
- **FILEMGR-10 — preview + thumbnails + built-in viewers.** Lazy cached thumbnails
  (images/video, capped); the preview pane + spacebar quick-look; built-in image/text/media
  viewers (no external spawn); remote lazy/on-demand.
- **FILEMGR-11 — the operation dialogs.** Confirm-delete, the conflict dialog, the
  Properties/permissions dialog (rwx grid + octal + owner/group, chown-gated), and the
  **typed-arming** dialog for remote/escalated destructive ops (names the node+path).
- **FILEMGR-12 — mesh integration.** Right-click Send-To a peer; **Send in Chat** (reuse the
  file message-kind); cut/copy/paste over the shared shell clipboard (cross-surface paths).

**Serialization**: FILEMGR-1 first (the FileOps core everything uses); 2/3/4 build on 1
(backend, parallelizable — distinct modules); 5→6→7 the mesh chain (worker → key → transfer);
8 the surface shell (needs 1's ops surfaced); 9 needs 5 (mount requests) + 8 (sidebar); 10/11/12
layer onto 8. The mackesd units (5/6) parallelize with the mde-files backend units (1/2/3/4);
the mde-files-egui units (8/9/10/11/12) serialize on the shared surface files (main/view).

## Acceptance (epic-level, runtime-observable)

1. Every locked operation works on a local path (copy/move/rename/delete-with-confirm/mkdir/
   symlink/chmod/properties/search/archive) with real progress + conflict handling; `demo_data`
   is gone.
2. The Mesh sidebar lists every enrolled peer with a live presence pip; navigating into an
   online peer mounts its home over sshfs and browses it like a local folder; offline peers are
   honestly greyed.
3. The "access full filesystem" GUI action escalates a mount from home to `/`; a destructive op
   on a remote/escalated mount demands the typed-hostname arming.
4. Copying a file from node A to node B transfers directly (not double-hopped through this node),
   shown as one queued transfer with progress.
5. A dropped mount shows "reconnecting" and recovers when the node returns; a frozen mount never
   hangs the UI.
6. Views/tabs/dual-pane/DnD/thumbnails/preview all work; Send-To + Send-in-Chat + shared
   cut/copy/paste integrate.

## Risks / out of scope

- **Risks**: the shared-key blast radius (lock 13 — accepted; mitigate: node-sealed, overlay-only
  sshd bind, re-key path); sshfs freezes hanging a UI (lock 15 frozen-mount detection + bounded
  timeouts are the guard); permanent-delete + no-undo on a remote root (lock 19 typed-arming is
  the guard); sshfs WAN latency (lock 18 tuning + async).
- **Out of scope**: a trash/undo store (lock 3/6); password-protected archives (lock 10);
  external app launching (lock 23); a standalone window (lock 25 — panel only); CA-signed SSH
  certs (lock 13 chose shared-key); write-caching/prefetch layer (lock 18 chose tuned-sshfs, not
  a local cache).
