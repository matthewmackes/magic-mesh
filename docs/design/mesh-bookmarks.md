# Mesh Browser + Bookmarks — synced bookmarks, a sandboxed Servo browser, an ad-blocker service (BOOKMARKS-1..10)

> **Status: LOCKED 2026-07-02** — a 100-question operator survey (+ the earlier core
> survey), evolved live from a "Vault" brief. A new **`Surface::Bookmarks`** browser in the
> magic-mesh shell: import bookmarks from every major browser, manage them as a
> mesh-synced CRDT collection, browse the web in an **out-of-process sandboxed Servo
> engine**, all filtered by a **mesh-wide ad-blocker service**.
>
> **⚠️ PIVOTS captured (operator, mid-survey):**
> - **NO credential/password handling at all.** The Firefox NSS/logins/Primary-Password
>   half of the original brief is **dropped entirely** — bookmarks only, no password
>   reading, no `libnss3`, no saving passwords anywhere.
> - The Servo side is a **real interactive browser** (links, tabs, address bar,
>   back/forward, JS-on), not a read-only preview.
> - **Carbon Design** for the GUI (via the platform §4 mde-egui `Style` tokens).
> - Build **all-at-once**, **fleet-wide default-on** at release; Servo **always in the
>   workspace build**.

## The locks (grouped)

### Bookmarks — model + CRDT
| # | Lock |
|---|------|
| Q1 | Per-bookmark **UUID** minted at creation (the CRDT key). |
| Q2 | **Strict tree**, one parent per item. |
| Q3 | **Fractional indexing** (LSEQ-style order keys) for manual order — drag = one op, no renumber storms. |
| Q4 | **No tombstones — LWW on delete** (operator's call; accepted resurrection edge). |
| Q5 | **Hybrid Logical Clock** (wall+counter+node-id) decides LWW. |
| Q6 | Favicons in a **content-addressed, deduped store**, synced lazily. |
| Q7 | Fields: title, url, favicon-ref, tags, notes, added+modified, source. |
| Q8/Q61 | **One shared collection for the whole mesh** (all users). |
| Q64 | Each op carries **author user-id + node-id**; the worker only writes ops for the local authenticated user. |

### Import
| # | Lock |
|---|------|
| Q9 | **Manual file/dir picker + format auto-detect** (no auto-scan). If a dir holds multiple profiles, list them; else import the file. |
| R1/Q13 | Importers: **Firefox `places.sqlite` (bookmarks only), Chromium `Bookmarks` JSON, universal Netscape HTML** (Safari via its HTML export — no native plist). |
| Q11 | Firefox tags → `tags[]` on the bookmark (not tag-folders). |
| Q12 | Chromium roots → named subfolders (`Bookmarks Bar`/`Other`/`Mobile`) under `Imported/<Browser>`. |
| Q14 | **Full HTML parse** (nested folders + titles + dates + tags/icons). |
| Q15 | Dedup by **normalized URL** (lowercase host, strip trailing-slash + fragment + tracking params). |
| Q16 | Import collision → **keep existing in place**, refresh title/favicon, don't duplicate; add only new URLs. Idempotent re-import. |

### Mesh sync
| # | Lock |
|---|------|
| Q17 | **Per-node append-only op segments** in a Syncthing-shared folder; peers replay + CRDT-merge (no file conflicts). |
| Q18 | **All enrolled nodes** by default, opt-out per node. |
| Q19 | New node **replays all segments** to converge (no special bootstrap RPC). |
| Q20 | **Snapshot + prune superseded ops**, bounded tail (prune horizon > max offline window). |
| Q21 | **Offline-first**: edit freely offline, subtle freshness indicator, silent CRDT converge on reconnect. |
| Q22 | **In-memory indexed tree + virtualized render + incremental merge** (snappy at 10k+). |
| Q23 | **Silent LWW** but a transient "changed elsewhere" flag on the item. |
| Q24 | Rides the **existing encrypted mesh Syncthing substrate**; all enrolled members read/write. |
| Q90 | Ops **flushed periodically** (in-memory + periodic flush durability). |
| Q91 | Sync down → **full local editing continues**, "not syncing" indicator, auto-resume. |

### Manager UI (Carbon §4)
| # | Lock |
|---|------|
| Carbon | **Carbon Design via mde-egui `Style` tokens** (type/spacing/DANGER-WARN-OK color ramp) — no raw hex; matches the platform §4 discipline + the honest-empty-state idiom. |
| Q25 | **Left folder-tree + main list + preview/detail pane** (three regions). |
| Q26 | Add via **+ dialog, add-from-preview, paste-URL, keyboard shortcut**. |
| Q27 | Search = **title + URL** (live). |
| Q28 | **Tags kept from import (searchable-later), no active tags UI** (trimmed). |
| Q29 | **Drag reorder + drag-onto-folder move** (incl. multi-select), reuse the shell DnD. |
| Q30 | Folders: **create/rename/nest/delete** (confirm on non-empty), reorder. |
| Q31 | **Manual order default + optional sort-by** name/url/added/recent (view pref, per-folder). |
| Q32 | **Ctrl/Shift multi-select → bulk** move/delete/open-all/copy-URLs. |
| Q79 | Bookmark open: **double-click → new browser tab**; single-click selects. |
| Q89 | **Clean distinct honest error/empty states** (match the platform `honest_*_state` idiom + Style DANGER/WARN). |

### The Servo browser (interactive)
| # | Lock |
|---|------|
| Q2(orig) | **Out-of-process Servo helper** (`mde-web-preview` bin); surface talks IPC + gets the texture. |
| Q33 | A **real interactive browser** — clickable links, navigation, back/forward, address bar. |
| Q34 | **Multiple tabs** (one Servo instance, per-tab WebViews). |
| Q35 | Full chrome: **address bar + back/forward/reload/stop + progress + title + ad-block toggle/count**. |
| Q36 | **JS on by default** (interactive browser). |
| Q72 | Media/canvas/WebGL: **support what Servo supports, honest degrade**. |
| Q65 | **Track Servo monthly** releases. |
| Q66 | **GPU required** (wgpu); policy disables the browser on GPU-less nodes. |
| Q68 | **Lazy first-launch** ("starting browser…") + keep one warm helper. |
| Q4(orig) | Preview/browse **off by default until the user acts**; URL allowlist; honest offline. |

### Security / sandbox / privacy
| # | Lock |
|---|------|
| Q37 | Helper sandbox: **user namespace + seccomp-bpf + minimal caps + no-new-privs**. |
| Q38 | **Network egress UNRESTRICTED** (only the ad-blocker filters) — ⚠️ **accepted residual risk** (a compromised engine keeps network reach; full containment would need the declined egress proxy). |
| Q39 | FS: **read-only minimal rootfs + private tmpfs; NO home / mesh-keys / data**. |
| Q40 | Blast radius: **contained to the throwaway sandbox** (no identity, no persistence, killed per-session). |
| Q53 | **One sandboxed process per tab**, torn down per session. |
| Q54 | **Zero telemetry** (no engine phone-home; only page loads). |
| Q55 | **System-CA TLS**, HTTPS-preferred with honest warnings on plain-HTTP / cert errors. |
| Q56 | **Downloads → quarantine folder; no uploads; no file://**. |
| Q67 | **Per-tab cgroup memory+CPU cap + layout-thread cap**; kill + honest "used too much". |
| Q69 | **Deny all sensitive web permissions** (geo/cam/mic/notif/…), no prompts. |
| Q70 | Popups → **new tab**, auto-popups blocked. |
| Q71 | **Standard clipboard API** (operator's call — noted minor mesh-secret-in-clipboard leak risk). |
| Q73 | **First-party session cookies, block third-party, clear on close**. |
| Q74 | **In-session back/forward only, NO persistent history** (private-by-default). |
| Q75 | **Referrer-trim (origin-only) + basic fingerprint reduction**. |
| Q76 | **Generic non-identifying UA** (never leaks node/mesh). |
| Q80 | **No per-URL browsing audit** (privacy-first); only ad-blocker stats + policy/security events logged. |
| Q95 | `unsafe` **confined to named modules** (Servo FFI / shm / sandbox setup), review comments, denied elsewhere. |

### Ad-blocker service
| # | Lock |
|---|------|
| R6 | A mackesd **ad-filter service** + `adblock-rust` in the helper, blocking at Servo's network layer. |
| Q41 | **EasyList + EasyPrivacy + uBlock base bundled + operator-addable custom lists**. |
| Q42/Q57 | **Network request-blocking + cosmetic element-hiding** (injected user-stylesheet, JS-off-safe). |
| Q43/Q60 | **Leader compiles the serialized engine once, replicates the blob** over Syncthing; nodes load prebuilt. |
| Q44 | **Per-site allowlist synced mesh-wide**, block-on-by-default. |
| Q59 | **Best-effort anti-adblock** lists, no arms race. |
| Q58 | **Per-page count + global/per-domain stats**, fleet-rolled-up. |
| Q77 | **Mesh/overlay domains always allowed + never ad-blocked** (built-in allowlist). |
| Q92 | Fetch fail → **last-synced then bundled seed lists**, honest staleness indicator. |

### Mesh service architecture + fleet
| # | Lock |
|---|------|
| Q45 | **Separate `bookmarks` + `adfilter` mackesd workers** (Syncthing-synced); the Servo helper is a **spawned bin**, not a worker. |
| Q46 | **Fleet policy** (mesh-wide): per-role browser enable/disable, force ad-blocker on, URL allowlist, custom lists. |
| Q62 | Policy **enforced by the local worker/launcher** (not just hidden in the UI). |
| Q63 | Disable → **stop sync + hide surface, retain local data** (re-enable resumes); separate explicit purge. |
| Q47 | **Leader / Media-style role fetches** filter updates; **failover** to another eligible node. |
| Q48 | Per-node service state + stats **published to the Workbench fleet view**. |
| Q78 | Browser is a **local shell surface only** (not injected into VMs); the collection still syncs mesh-wide. |

### Servo engineering + IPC
| # | Lock |
|---|------|
| Q49 | **Dedicated Unix-domain socket per helper session**, typed length-prefixed messages (not the mesh Bus). |
| Q50 | **Shared memory (shm/dmabuf)** for the frame; surface uploads to a texture on paint-ready (not every frame). |
| Q51 | Crash → **honest "page crashed" state + respawn on reload**; other tabs unaffected. |
| Q52 | **Servo always in the workspace build** (⚠️ accepted farm build-time + image-size cost). |

### Packaging + testing + rollout
| # | Lock |
|---|------|
| Q81 | Declare Servo's **runtime libs as RPM requires** (mesa/vulkan/fontconfig/freetype/…); bundle seed filter lists; **no firefox/nss deps**. |
| Q82 | Ship a **confined enforcing SELinux domain** for the helper. |
| Q83 | Helper **in the base bootc image**, runtime-gated by policy. |
| Q84 | Produce **design doc + THREAT_MODEL + CHANGELOG** (Servo pin + accepted risks). |
| Q93 | Layered tests: **CRDT convergence property-tests + import fixtures + headless Servo frame-arrival + egui snapshot**. |
| Q94 | **Standard build/test/clippy gates** (operator declined the extra security-review checklist). |
| Q96 | **§7 runtime-reachable, no stubs**; live edges honestly gated. |
| Q97/Q99 | Build **all-at-once**; **fleet-wide default-on** at release. |
| Q100 | Out-of-scope confirmed (below). |

## Architecture

```
Surface::Bookmarks (mde-shell-egui, Carbon §4)          mackesd (mesh tier)
┌───────────────────────────────────────────┐          ┌───────────────────────────────┐
│ folder tree · list · detail/browser pane   │ action/  │ bookmarks worker              │
│ browser: tabs · addr bar · back/fwd · adblk│ bookmarks│  · UUID/tree/HLC CRDT ops      │
│ import (file-pick) · search · bulk · DnD    │──────────▶  · per-node segments (Syncthing)│
│ honest error/empty states                   │◀──────────  · replay-merge · snapshot-prune│
└──────┬──────────────────────┬───────────────┘ state/*  └───────────────────────────────┘
       │ FileOps? no.         │ per-session Unix socket + shm/dmabuf
 mde-bookmarks (lib)          │                          ┌───────────────────────────────┐
  · tree + CRDT + importers   │            action/adfilter│ adfilter worker               │
  · content-addr favicons     ▼                          │  · lists+custom · leader-compile│
              mde-web-preview (sandboxed bin, per tab)    │  · serialized-engine blob sync │
              · Servo (JS-on, tabs, nav) · adblock-rust   │  · per-site allowlist (synced) │
              · userns+seccomp+SELinux · cgroup caps      │  publishes state/adfilter/*    │
              · zero-telemetry · deny-perms · no-persist  └───────────────────────────────┘
              · shm frame → surface texture               (fleet policy governs both, worker-enforced)
```

## The units (BOOKMARKS-1..10)

- **BOOKMARKS-1 — model + CRDT.** `mde-bookmarks` lib: UUID tree, HLC, fractional-index order,
  LWW-delete, op set + merge, op attribution (user+node), content-addressed favicon store. Pure,
  property-tested for convergence.
- **BOOKMARKS-2 — bookmarks worker + mesh sync.** mackesd `bookmarks` worker: per-node op segments
  over the encrypted Syncthing share, replay-merge, snapshot+prune, in-memory index, periodic-flush
  durability, offline-first, `action/bookmarks/*` + `state/bookmarks/*`. Two-node convergence test.
- **BOOKMARKS-3 — importers.** Firefox sqlite (bookmarks only) / Chromium JSON / universal HTML;
  file/dir picker + auto-detect; FF-tags→tags, Chromium-roots→subfolders; normalized dedup;
  idempotent `Imported/<Browser>`. Fixture-tested per format. **No logins/cookies/history.**
- **BOOKMARKS-4 — Surface::Bookmarks UI (Carbon §4).** Three-region; folder CRUD; manual+sort order;
  title+url search; multi-select bulk; DnD; add paths; favicon render; honest error/empty states
  (platform idiom). egui snapshot tests.
- **BOOKMARKS-5 — the `mde-web-preview` Servo browser (bin).** Interactive Servo (JS-on, tabs, nav,
  media), the OS sandbox (userns+seccomp+SELinux, process-per-tab, cgroup caps), zero-telemetry,
  system-CA TLS, deny-perms, cookie/history/UA policy, lazy+warm launch, GPU-required. Headless test:
  about:blank → a frame on the shm channel.
- **BOOKMARKS-6 — IPC + shm texture bridge.** Per-session Unix socket (typed frames) + shm/dmabuf
  frame transport; input forwarding (pixels_per_point); crash→honest-state+respawn; navigation chrome
  wiring.
- **BOOKMARKS-7 — the ad-filter service + engine.** mackesd `adfilter` worker (bundled+custom lists,
  leader-compile serialized engine, Syncthing sync, per-site allowlist synced, anti-adblock, staleness
  fallback, `state/adfilter/*`) + `adblock-rust` in the helper (network + cosmetic user-stylesheet,
  mesh-domain exemption, blocked stats). Rule-match folds unit-tested.
- **BOOKMARKS-8 — fleet policy + observability.** Mesh policy (per-role browser enable, forced
  ad-blocker, URL allowlist, custom lists) worker/launcher-enforced; per-node state+stats to the
  Workbench fleet view.
- **BOOKMARKS-9 — packaging + docs.** Servo runtime-lib RPM requires + bundled seed lists; confined
  enforcing SELinux domain; base-image + policy gating; THREAT_MODEL + CHANGELOG. DoD: `rpm -qlp`
  ships `mde-web-preview`; base workspace build green.
- **BOOKMARKS-10 — mesh integration.** Send-in-Chat a bookmark (reuse the NOTIFY-CHAT message-kind),
  copy-URL to the shell clipboard, add-bookmark-from the current page.

**Serialization**: 1 first; 2/3 on 1 (parallel — worker vs importers); 4 on 1/2's state; 5 (Servo
bin) independent — the heavy long pole (route to BigBoy); 6 needs 4+5; 7 = adfilter worker (parallel)
+ the helper engine (after 5); 8 after 2/7; 9 after 5; 10 onto 4. Most of 1/2/3/4/7-worker/8/10 is
Servo-free; only 5/6/7-engine/9 carry the Servo compile.

## Acceptance (epic-level, runtime-observable)

1. Import from a picked browser file lands bookmarks under `Imported/<Browser>`, normalized-deduped,
   idempotent; logins/cookies/history never touched.
2. Edits on node A converge on node B (CRDT); concurrent edits converge; offline edits merge on
   reconnect; sync-down keeps local editing working.
3. The browser navigates real sites (links/tabs/back-fwd/address bar, JS on), runs in the OS sandbox
   (verify userns+seccomp active, process-per-tab), with zero telemetry + deny-all-perms + no
   persistent history/cookies.
4. The ad-blocker (default on) blocks ad/tracker requests (count>0) + hides cosmetic elements while
   the page renders; lists are leader-compiled + synced mesh-wide; per-site allowlist works; mesh
   domains are never blocked.
5. Fleet policy disables the browser on a disallowed role (worker-enforced) + forces the ad-blocker;
   per-node stats appear in the Workbench.
6. The base workspace/RPM build is green; the RPM ships `mde-web-preview` + the SELinux policy + seed
   lists; a crash in one tab doesn't take down the shell.

## Risks / out of scope

- **Risks / accepted tradeoffs**: **unrestricted network egress** (Q38 — a compromised engine keeps
  network reach; documented residual risk); **standard clipboard API** (Q71 — page-readable clipboard
  on a node holding mesh secrets); **Servo always-in-build** (Q52 — farm build-time + image size);
  **track-monthly Servo** (API churn); LWW-delete resurrection edge (Q4); Servo rendering fidelity
  (younger engine — heavy sites/media may not fully work). The sandbox (userns/seccomp/SELinux/
  cgroups/no-FS/no-identity, process-per-tab, zero-telemetry, deny-perms) contains the rest.
- **Out of scope**: ANY credential/password/cookie-jar/history persistence; uploads (downloads only to
  quarantine); the browser inside VM sessions; live-watch import (on-demand only); a standalone window
  (shell surface only); requiring Firefox/NSS installed; CA-signed anything; the egress proxy
  (declined); perfect anti-fingerprint/anti-adblock.
