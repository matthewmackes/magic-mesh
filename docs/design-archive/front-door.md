> **HISTORICAL / SUPERSEDED (2026-07-22):** interface-paradigm design retired by the PLATFORM-INTERFACES standard (Apple-HIG-principled Construct + Car); see [docs/design/platform-interfaces.md](../design/platform-interfaces.md). Archived; do not implement from this document.

# FRONTDOOR — the Magic Mesh Front Door (App Menu redesign)

> **HISTORICAL / SUPERSEDED IN PART (2026-07-19):** the Front Door concept lives on, but this doc's implementation is the retired `mde-workbench` launcher. The live desktop is the egui-native, DRM-native shell `mde-shell-egui` — see [`quasar-vdi-desktop.md`](quasar-vdi-desktop.md). Read the `mde-workbench`/iced crate references below as historical.

> Design locks from the 100-question survey, 2026-06-25. Supersedes the old
> `mde-workbench` launcher. Authority: `AI_GOVERNANCE.md` §0 (Secure, Simple,
> No-Fixed-Center), §4 (Carbon), §6 (mesh boundary), §7 (Definition of Done).

## Vision

The **Front Door** is the primary interface to the mesh and the local OS — a single
surface that is **Windows 10 Start** when summoned as a panel and **iPadOS home**
when toggled full-screen, rendered fast on a custom **wgpu** path (the 4-second menu
dies). It is Carbon-skinned, expressive, and alive: it floats over the blurred live
mesh wallpaper, and a proactive AI agent — **Copilot** — lives *in the tiles*,
reading the whole mesh and able to operate it end-to-end with one click. DevOps and
Data Center are front-and-center. The feel is powerful, seamless, and ceremony-free.

## The 100 locks

### Layout & shell
| # | Decision | Lock |
|---|---|---|
| Q1 | Layout paradigm | **Two-pane: left rail + live-tile grid** (Win10 Start) |
| Q86/89 | Full-screen mode | **iPadOS home**: paged rounded-icon grid + widgets, **no dock** |
| Q5 | Left rail | Identity + power + pinned + **DevOps + Data Center** sections |
| Q29 | Window form | **Panel default + full-screen toggle** |
| Q85 | Summon | **Super / Win key** |
| Q98 | Omnibox position | **Top of panel, full-width** |
| Q72 | Binary | **Keep `mde-workbench` name** (drop-in), RPM unchanged |
| Q41 | Where built | **Refactor `mde-workbench` in place** |
| Q57 | Rollout | **Replace the old launcher directly; remove at parity** |
| Q84 | Backdrop | **Blur/dim the live mesh wallpaper** behind the panel |

### Look, motion, performance
| # | Decision | Lock |
|---|---|---|
| Q13 | Render path | **Custom wgpu renderer for the grid** (embedded in the iced shell) |
| Q4 | Perf approach | **Rebuild on a lighter render path** |
| Q28 | Speed budget | **<1s** open-to-interactive |
| Q60 | Perf tracking | **None** (the rebuild carries it; no CI perf gate) |
| Q92 | Tile loading | **Skeleton placeholders** while data streams (no layout shift) |
| Q23 | Theme | **Follow OS / auto** (light/dark) |
| Q65 | Accent | **Carbon Blue 60** |
| Q24 | Motion | **Expressive** (characterful) |
| Q75 | Polish | **Rich** — animations + sound + haptics |
| Q77 | Sound | **Subtle Carbon cues, mutable** |
| Q80 | Density | **Comfortable default + compact toggle** |

### Tiles & widgets
| # | Decision | Lock |
|---|---|---|
| Q6 | Default tiles | **Live mesh/DevOps widgets + app launchers** |
| Q99 | Widget set | Mesh map, build/farm, alerts, node health, Copilot, system |
| Q21 | Tile sizes | **4 sizes (small/med/wide/large), resizable, snap-grid** |
| Q22 | Tile updates | **Event-driven via mde-bus** + slow-poll fallback |
| Q42 | Tile data source | **mde-bus topics, backed by mackesd workers** |
| Q45 | Tile click | **Opens a detail view** |
| Q49 | Detail view | **An actions menu** |
| Q79 | Arrangement | **Settings-panel managed** |
| Q35 | Grouping | **Auto-grouped by category** |
| Q59 | Custom tiles | **Copilot authors a tile from any data/command** |
| Q31 | Catalog | **Full suite** (incl. music/files/voice) |
| Q40 | Existing apps | **Stay standalone**; Front Door launches them |
| Q64 | Recents | **None** |

### Search
| # | Decision | Lock |
|---|---|---|
| Q20 | Scope | **Unified: apps + files + mesh + AI answers** |
| Q47 | Search + AI | **Instant local results; AI answer streams in below** |
| Q81 | Ranking | **AI ranks everything** |
| Q58 | Command syntax | **None** (pure natural input) |

### The AI — Copilot
| # | Decision | Lock |
|---|---|---|
| Q39 | Name | **Copilot** |
| Q2 | Modality | **Ambient / contextual** (no central bar) |
| Q7 | Surface | **Proactive inline suggestions** |
| Q19 | Suggestion home | **Inline on the relevant tile** |
| Q61 | Proactivity | **Moderate** — high-confidence, high-impact only |
| Q9 | Engine | **Context feed → codex → ranked suggestion cards** |
| Q62 | Learning | **Explicit thumbs up/down only** |
| Q30 | Follow-up | **Click a suggestion → task-scoped mini-conversation** |
| Q88 | Memory | **Full durable transcript** |
| Q37 | Value | **Ops fixes + optimizations** |
| Q50 | Tone | (implicit) expert ops peer |
| Q55 | Voice | **Optional push-to-talk + spoken summaries** (mde-voice-hud) |
| Q33 | Offline | **Graceful degrade** — everything but AI keeps working |

### Codex backend
| # | Decision | Lock |
|---|---|---|
| Q3 | Backend shape | **mackesd `copilot` worker** wrapping codex |
| Q43 | Worker | **New mackesd `copilot` worker + bus ask/act topic** |
| Q73 | Host | **The leader only** |
| Q78 | Failover | **Follows the leader; state in etcd; seamless** |
| Q14 | Codex mode | **Non-interactive `exec` per request, sandboxed** |
| Q100 | Source | **External dependency, pulled at runtime** |
| Q15 | Auth | **Leader-managed sealed mesh secret** |
| Q93 | Rotation | **Set once** |
| Q87 | Model tier | **Tiered: fast for suggestions, strong for actions/edits** |
| Q16 | Grounding | **System state dumped as context** |
| Q69 | Context scope | **Full mesh state every request** |
| Q95 | Data reach | **Everything incl. file contents** |

### Capabilities & execution
| # | Decision | Lock |
|---|---|---|
| Q8 | OS reach | **Full ops + confirm gate on destructive** |
| Q52 | Copilot tools | **Everything incl. editing configs/code** |
| Q53 | Code edits | **Propose diff → review + apply → git-committed** |
| Q17 | Execution path | **Via a mackesd action worker** (typed, audited) |
| Q10 | Confirm gate | **Preview/diff + 1-click; typed-confirm for high-risk** |
| Q44 | Confirm preview | **Commands + target node(s) + effect + dry-run** |
| Q46 | Guardrails | **Trust the model + audit** (gate guards destructive only) |
| Q18 | Mesh reach | **Whole-mesh broadcast** (default) |
| Q70 | Broadcast safety | **Same confirm as single-node** (blast radius shown) |
| Q32 | Cross-node | **A tile can target any node; result returns** |
| Q74 | Remote launch | **Ops headless on the node; GUI apps local on remote data** |
| Q54 | Provisioning | **Drives the tofu/autoscaler** (operator-gated apply) |
| Q67 | Action failure | **Inline error + Copilot diagnoses + fix/retry** |
| Q66 | Long ops | **Progress tile/strip + cancel + notify** |
| Q25 | Authz | **Role-gated**: Wkstn/Server full, Lighthouse read-only |
| Q26 | Audit | **Suggestion + action + confirm → mesh audit plane** |

### Surfaces — DevOps & Data Center
| # | Decision | Lock |
|---|---|---|
| Q11 | DevOps lead | **Build/CI + 1-click deploy/rollback + farm utilization** |
| Q50d | DevOps actions | build, deploy, rollback, logs, rerun-failed |
| Q12 | Data Center lead | **Live topology + per-node health + 1-click node actions** |
| Q51 | DC actions | **Full lifecycle** incl. provision/destroy + cutover helpers |
| Q38 | Alerts | **Alerts tile + AI triage** (cluster, explain, one-click fix) |
| Q63 | Notifications | **Keep notifyd separate** |
| Q36 | Power | **Local power + a separate mesh-power tile** |

### Platform, identity, prefs
| # | Decision | Lock |
|---|---|---|
| Q48 | Settings | **In-menu panel + mesh-synced prefs** |
| Q56 | Prefs sync | **etcd coordination plane** |
| Q82 | Multi-user | **Single shared layout per node** |
| Q91 | Lock | **Locks with the session; actions need unlock** |
| Q83 | Telemetry | **None** |
| Q94 | Accessibility | **Visual-only for now** |
| Q34 | Keyboard | **Mouse-primary, basic keyboard** |
| Q68 | Persistent applet | **No** — full menu only (summoned) |
| Q27 | First run | **Guided: AI greets + auto-builds tiles from the mesh** |
| Q71 | Onboarding tour | **None** (discoverable) |

### Delivery
| # | Decision | Lock |
|---|---|---|
| Q76 | Scope | **Everything at once** (one release) |
| Q97 | Build order | **Parallel tracks merged together** |
| Q96 | DoD | **Build + test + Carbon-token clean + runtime-reachable smoke** |

## Architecture

```
            ┌──────────────────────── mde-workbench (refactored, drop-in) ───────────────────────┐
            │  iced shell  +  CUSTOM WGPU GRID RENDERER  (kills the 4s; <1s; skeletons)           │
            │  PANEL  = Win10 Start (left rail + live-tile grid)   |   FULL-SCREEN = iPadOS home   │
            │  Carbon, follow-OS theme, Blue 60, expressive motion, rich polish, mesh-wallpaper bg │
            │  ┌ left rail: identity · power(+mesh tile) · pinned · DevOps · Data Center ┐         │
            │  ┌ top omnibox: unified apps+files+mesh+AI, AI-ranked, instant + AI-stream ┐         │
            │  ┌ tiles/widgets: 4 sizes, snap-grid, auto-grouped, click→actions menu      ┐        │
            │       │ subscribe (event-driven + slow poll)                                         │
            └───────┼──────────────────────────────────────────────────────────────────────────┘
                    │ mde-bus topics
            ┌───────▼─────────── mackesd workers (per node) ───────────────────────────────────┐
            │  tile-data workers · action worker (typed, AUDITED) · copilot worker (LEADER only) │
            │     copilot: codex `exec` per req (external, runtime) · sealed mesh-secret key      │
            │     · tiered model · full-mesh-state context · tools: read/run/script/edit-code     │
            │     · proactive ranked suggestions → bus → inline tile cards (thumbs feedback)       │
            └───────┼──────────────────────────┼────────────────────────────┼────────────────────┘
                    │ etcd (prefs, transcripts, │ mesh audit plane           │ tofu/autoscaler
                    │  leader, copilot state)    │ (suggestion+action+confirm)│ (provision/destroy)
```

- **One surface, two modes.** `mde-workbench` refactored in place; the slow launcher
  is deleted at parity. Super/Win summons the **panel** (Win10 two-pane); a toggle
  goes **full-screen** (iPadOS paged icon-grid + widgets, no dock).
- **Render.** A custom **wgpu** renderer draws the tile/icon grid inside the iced
  shell — the perf rebuild. Target <1s cold; skeleton placeholders; no layout shift.
- **Tiles** subscribe to **mde-bus** topics fed by per-node mackesd workers; 4
  resizable sizes on a snap-grid, auto-grouped, arranged in the settings panel;
  click opens a detail **actions menu**. Copilot can author new tiles on request.
- **Copilot** runs as a **mackesd worker on the leader** (state in etcd, follows
  leadership). It wraps **openai/codex** (external, pulled at runtime) in sandboxed
  `exec` per request, keyed by a **sealed leader-managed mesh secret**, tiered model,
  grounded by a **full-mesh-state context dump** (reads everything incl. files).
  It is **proactive** (moderate): ranked suggestion cards appear **inline on the
  relevant tile**; clicking one opens a task **mini-conversation** (durable transcript,
  thumbs feedback). Offline → graceful degrade (everything else still works).
- **Execution.** Approved actions run through a **mackesd action worker** (typed,
  audited). Default reach is **whole-mesh broadcast**; the standard confirm applies
  with the **blast radius shown**, and a **preview/diff (commands + targets + effect +
  dry-run) with typed-confirm guards destructive ops**. Otherwise low-friction
  (trust + full audit to the mesh audit plane). Role-gated (Lighthouse read-only).
- **DevOps** = build/CI + 1-click build/deploy/rollback/logs/rerun + farm util.
  **Data Center** = live topology + node health + 1-click full lifecycle (join/drain/
  restart/**provision/destroy** via the tofu/autoscaler) + cutover helpers.
- **Alerts** tile with AI triage; notifyd stays separate. **Voice** optional via
  mde-voice-hud. Prefs/layout/transcripts in **etcd**. Locks with the session.

## Build tracks (parallel, merged into one release)

1. **Shell + perf** — wgpu grid renderer, the two render modes, <1s open, skeletons.
2. **Tiles + data** — tile model, mde-bus data workers, widget set, detail actions menu.
3. **Search** — unified omnibox, AI-ranked, instant-local + AI-stream.
4. **DevOps surface** — build/CI tiles + 1-click pipeline actions + farm util.
5. **Data Center surface** — topology + health + node-lifecycle actions (tofu/autoscaler).
6. **Copilot worker** — mackesd leader worker, codex exec, sealed key, bus ask/act, suggestions.
7. **Action worker + confirm gate** — typed/audited execution, preview/diff, whole-mesh, audit.
8. **Copilot capabilities** — read/run/script + code-edit (diff→apply→git), failure diagnosis.
9. **Platform** — settings + etcd prefs sync, theme/motion/sound/polish, wallpaper backdrop, lock.
10. **Rollout** — replace the old launcher, parity check, remove dead code.

## Acceptance (DoD — §7, visual gate lifted)

- Builds + `cargo test` green; `cargo clippy` clean; **no raw hex / scattered metrics**
  outside `mde-theme` tokens (§4); `lint-mesh-boundary.sh` clean (§6).
- **Runtime-reachable smoke**: the binary launches both modes; tiles render with live
  data; search returns ranked results; a DevOps and a Data Center 1-click action each
  execute through the action worker and log to the audit plane; a Copilot suggestion
  appears and its action runs through the confirm gate.
- Cold open **<1s** (the 4s is gone) — measured by hand at smoke time (no CI perf gate).
- No stubs / `todo!()` / dead modules; every surface reachable from the binary.

## Risks

- **wgpu-in-iced integration** is the hardest unknown — embedding a custom GPU grid
  renderer in the iced shell. De-risk first (track 1) before layering features.
- **codex pulled at runtime** + **reads file contents** + **whole-mesh broadcast with
  standard confirm** is a powerful, low-friction posture. Mitigations in scope: the
  destructive-op typed-confirm + dry-run preview, role-gating (Lighthouse read-only),
  and full audit. Prompt-injection on a file-reading, code-editing agent is a live
  risk accepted by the "trust + audit" lock — the audit plane is the backstop.
- **Leader-only Copilot** concentrates AI on one node; mitigated by etcd state +
  seamless follow-the-leader, but a leaderless window pauses AI (UI degrades, not breaks).
- **"Everything at once"** is a large release; the parallel tracks keep each
  independently testable so integration is incremental even though the cut is one.

## Out of scope (this epic)

- Accessibility beyond visual (deferred — Q94); deep keyboard nav (Q34 mouse-primary).
- Telemetry/usage analytics (Q83 none); recents/history (Q64 none); onboarding tour (Q71).
- Persistent taskbar applet (Q68); notifyd replacement (Q63 stays separate).
- Per-user isolation (Q82 single shared layout); codex key rotation tooling (Q93 set-once).
