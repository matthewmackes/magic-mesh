# E12 — Forked-COSMIC + Magic-Mesh → one egui-native mesh desktop OS

> **Status:** LOCKED (design) · 2026-06-30 · 4-round / 16-question `/plan` survey.
> **Supersedes:** the Cosmic-era desktop locks (`AI_GOVERNANCE.md` §4/§5/§6, E11).
> **Series:** opens **MCNF 12.0** (current tip: 11.2.0). Codename **"Quasar"**
> (operator-confirmed 2026-06-30).
> **Authority:** Memory > `AI_GOVERNANCE.md` > this doc > `docs/WORKLIST.md` body
> (newest wins). Tasks: `docs/WORKLIST.md` → `## E12` section.

## What this is

A new platform that **merges the upstream COSMIC desktop source and the
Magic-Mesh (MCNF) platform into one forked, mesh-native Fedora-Cosmic desktop
OS, with its entire UI refactored from libcosmic/iced to egui.** It is the
**E12 pivot**: where E11 *ended* the labwc desktop and made MCNF a tenant of
upstream Cosmic, E12 **forks Cosmic into the repo** and makes the desktop a
first-class, mesh-aware part of the platform — every surface an egui Wayland
client.

This is a clean break from three E11 locks: the libcosmic/iced toolkit (§4),
"Cosmic provides the desktop, MCNF integrates into it" (§5), and the
mesh/desktop boundary (§6). The mesh substrate (§1), the Bus (§2), the crypto
locks (§3), the Definition of Done (§7), the trust envelope (§8), the planes
(§9), and the build environment (§10) **carry forward unchanged.**

## Locked decisions (the survey)

| # | Fork | Lock |
|---|------|------|
| 1 | **Merge scope** | **Full COSMIC fork** — `cosmic-comp` (smithay compositor) + `cosmic-panel` + `cosmic-session` + `cosmic-settings` vendored into the repo, rebased on a pinned upstream. The platform *is* a desktop OS, compositor up. |
| 2 | **egui driver** | **One toolkit everywhere** — egui is the single rendering idiom for app windows, shell chrome, panel, and HUD/overlays. |
| 3 | **Carbon fate** | **Fresh egui-native design** — strict IBM Carbon is **retired**; §4 rewritten around the new language. |
| 4 | **Migration** | **Big-bang rewrite** — freeze the iced surfaces, rewrite all in egui, one cutover (no mixed-toolkit period). |
| 5 | **Render path** | **All egui as Wayland clients** — the forked `cosmic-comp` stays a *pure* compositor (no embedded UI); shell + panel + apps are **eframe** clients; the shell is independently restartable. |
| 6 | **Boundary** | **Layered tiers** — `mesh-substrate ⊂ platform-services ⊂ desktop-shell`; dependencies point only inward; lint-gated (new gate replaces `lint-mesh-boundary.sh`). |
| 7 | **Distribution** | **RPM layer on stock Fedora-Cosmic + integrated spin.** The egui clients install onto stock Cosmic as an RPM layer; the forked compositor ships in the spin. Headless Server/Lighthouse: mesh-only RPM set. |
| 8 | **Identity** | **Evolve MCNF → 12.0 series** (codename proposed "Quasar"); package/repo id stays `magic-mesh`. The "C" in MCNF now means *our forked Cosmic*, not *runs on Cosmic*. |
| 9 | **Design source** | **egui `Style` IS the source** — one shared `Style`/`Visuals` module; **no token crate, no raw-literal lint gate.** |
| 10 | **Motion** | **egui built-ins** (`animate_bool` / `ctx` animation) + a small shared duration+easing table; no bespoke motion module/gate. |
| 11 | **Accessibility** | **Deferred** — out of cutover scope (revisit post-stabilization). |
| 12 | **Mesh desktop** | **Compositor/session become mesh-aware** — per-peer workspaces, mesh overlays/HUD, desktop-state topics on the substrate (etcd + Syncthing); `mackesd` supervises the session. This is what earns the compositor fork. |
| 13 | **Packaging** | **Keep one-RPM + install-time role chooser** (headless + stock-Cosmic layer); **add a Fedora-Cosmic kickstart** for the integrated spin. gh-pages dnf repo unchanged. |
| 14 | **Sequencing** | **Shared `mde-egui` harness + `Style` first**, then fan the surface rewrites across the build farm in parallel. |
| 15 | **In-flight work** | **Abandon the iced GUI backlog entirely** (incl. unfinished PEERS GUI). All GUI effort → egui. Non-GUI mesh logic is untouched and **extended** (per lock 12). |
| 16 | **Governance** | **Full `AI_GOVERNANCE.md` rewrite** around E12; the Cosmic-era desktop text is archived as heritage. |

## Resulting architecture

```
                         MCNF 12.0 "Quasar" — one repo, three tiers
  ┌─────────────────────────────────────────────────────────────────────┐
  │  desktop-shell    (egui eframe Wayland clients — restartable)        │
  │    mde-shell-egui · mde-panel-egui · mde-workbench · mde-files ·     │
  │    mde-music · mde-voice-hud · mde-role-chooser   (all egui)         │
  │    + forked compositor:  cosmic-comp (pure, mesh-aware)             │
  │      cosmic-panel-fork · cosmic-session-fork · cosmic-settings-fork  │
  │                              │  deps point inward  ▼                 │
  ├─────────────────────────────────────────────────────────────────────┤
  │  platform-services   mackesd · mde-bus · magic-fleet · mde-enroll · │
  │                       session-supervisor · desktop-state worker      │
  │                              │  deps point inward  ▼                 │
  ├─────────────────────────────────────────────────────────────────────┤
  │  mesh-substrate      Nebula overlay · etcd · Syncthing · CA/KDC      │
  └─────────────────────────────────────────────────────────────────────┘
        lint: a dependency edge that points OUTWARD (substrate→services,
              services→shell) is a CI failure.
```

- **Compositor.** Fork `cosmic-comp` (smithay) into `crates/desktop/` (or a
  `cosmic/` vendor tree), pinned to a known-good upstream tag, rebased forward
  on a cadence. It stays a **pure compositor** — no UI is embedded in it. Its
  *fork value* is mesh-awareness (lock 12): per-peer workspaces, mesh overlays,
  desktop-state surfaced from etcd/Syncthing. `mackesd` supervises the session
  (start/restart/health) as a platform-service.
- **UI.** Every surface is an **eframe** (egui + winit + wgpu) Wayland client on
  a shared **`mde-egui`** harness: the client runner + the single shared
  `Style`/`Visuals` module + the duration/easing table. Because the clients are
  portable Wayland clients, the *same binaries* run on the forked compositor (the
  spin) and on **stock** upstream `cosmic-comp` (the RPM layer).
- **Design system.** Fresh, egui-native. The shared `Style`/`Visuals` module is
  the single source — a Rust module, not a token crate. No raw-literal lint gate.
- **Mesh substrate.** Unchanged (§1–§3). *Extended* by desktop-state Bus topics
  and a session-supervisor worker in `mackesd`.

### Crate-level disposition

| Today (libcosmic/iced) | E12 |
|---|---|
| `mde-workbench` | **Rewrite** in egui on `mde-egui`. |
| `mde-files` | **Rewrite** in egui (drop the cosmic-files fork lineage). |
| `mde-music` | **Rewrite** in egui. |
| `mde-voice-hud` | **Rewrite** in egui. |
| `mde-cosmic-applet` | **Rewrite** as `mde-panel-egui` widget(s) (egui panel client). |
| `mde-role-chooser` | **Rewrite** in egui (first-boot client). |
| `mde-theme` (Carbon tokens + `carbon.rs`/`palette.rs`/`motion.rs`/`density.rs`…) | **Retire.** Replaced by the `mde-egui` shared `Style`. Salvage only non-visual helpers if any. |
| `mde-card` | **Retire/absorb** — its render path is iced; logic (`probe`) folds into the relevant egui surface or a core crate. |
| — | **New:** `mde-egui` (harness + Style), `mde-shell-egui`, `mde-panel-egui`. |
| — | **New (forked):** `cosmic-comp`, `cosmic-panel`, `cosmic-session`, `cosmic-settings`. |
| `mackesd`, `mde-bus`, `magic-fleet`, `mde-enroll`, `mackes-*`, `kdc/*`, `mde-musicd`, `mde-notify`, `mde-role`, `mde-disclaimer` | **Keep** (non-GUI). `mackesd` gains the session-supervisor + desktop-state worker. |

### Lint-gate disposition

- **Retire:** `lint-carbon-tokens.sh`, `lint-motion.sh`, `lint-libcosmic-rev.sh`,
  `lint-no-cratesio-iced.sh` (all libcosmic/Carbon-specific).
- **Replace:** `lint-mesh-boundary.sh` → a **layered-tiers** gate enforcing the
  inward-only dependency direction across the three tiers.
- **Keep:** `lint-bus-names.sh` (§2), `lint-shared-substrate.sh` (§1).

## Acceptance criteria (epic-level, runtime-observable per §7)

1. The whole workspace builds with **zero libcosmic/iced dependency** (the four
   retired gates' subjects are gone); `grep -r libcosmic crates/` is empty.
2. The forked `cosmic-comp` builds from the in-repo source and starts a session;
   `cargo tree` shows it pinned to the recorded upstream rev.
3. Each of the 6 rewritten surfaces launches as an **egui Wayland client**,
   renders through the shared `Style`, and performs its core function live
   (Workbench drives the planes over the Bus; files browse+transfer; music
   plays; voice registers+dials; panel shows mesh status; role-chooser pins a
   role). No `todo!()`, no stub arms, no mockups (§7 holds).
4. Running on **stock** Fedora-Cosmic (RPM layer), the egui surfaces launch and
   work; running on the **forked** compositor (spin), the same binaries plus the
   mesh-aware behaviors (per-peer workspaces, mesh overlay) work.
5. The layered-tiers lint passes and **fails** on a planted outward edge
   (substrate→services or services→shell).
6. The kickstart produces a bootable spin with the forked compositor + the egui
   client set; the RPM still installs the role chooser + headless set on stock
   Cosmic.
7. `mackesd` supervises the desktop session (kill the shell client → it
   restarts) and publishes desktop-state to a Bus topic a peer can read.
8. `AI_GOVERNANCE.md` is the E12 rewrite; About/greeter reads `MCNF 12.0
   "<codename>"`; the package/repo id is still `magic-mesh`.

## Build sequencing (lock 14)

1. **`mde-egui` harness + shared `Style`** — the foundation (Wayland-client
   runner, `Style`/`Visuals`, duration/easing table, a11y-stub).
2. **Fork the COSMIC desktop stack** into the repo, pinned + building (parallel
   with 1).
3. **Fan the 6 surface rewrites** across the farm in parallel, each on the
   harness (worktree-isolated per §10.0).
4. **Mesh-aware compositor** + `mackesd` session-supervisor + desktop-state.
5. **Layered-tiers lint** + retire the four obsolete gates.
6. **Packaging** — kickstart spin + RPM/role-chooser update for the new client
   set.
7. **Decommission** the iced crates + `mde-theme`; remove the abandoned iced GUI
   backlog from the worklist.
8. **Identity** — 12.0 codename, About/greeter, version bump, governance rewrite
   landed.

## Risks

- **R1 — Compositor-fork maintenance.** Forking `cosmic-comp` (smithay) takes on
  a heavy upstream-tracking burden the E11 pivot deliberately shed. Mitigate:
  pin to a tag, keep the fork *pure* (mesh-awareness as additive layers), rebase
  on a cadence, document the delta.
- **R2 — Big-bang cutover risk.** All 6 surfaces switch toolkit at once with no
  fallback. Mitigate: harness-first de-risks the shared path; per-surface farm
  parallelism; each surface gated on §7 before the cutover merges.
- **R3 — Losing libcosmic's a11y/cosmic-text for free.** egui must re-prove text
  shaping; a11y is *deferred* (lock 11) — a known, accepted gap to revisit.
- **R4 — Dropping the token/lint discipline (locks 9/10).** No raw-literal gate
  means cross-surface visual drift is possible. Accepted as the §0-Simple lever;
  the shared `Style` module is the discipline.
- **R5 — Fresh design language (lock 3).** Retiring Carbon discards a large,
  tested design investment. Accepted; the new language is the new identity.
- **R6 — Abandoning unfinished iced work (lock 15)** strands partial PEERS GUI.
  Mitigate: the non-GUI PEERS data/Bus layer is preserved and feeds the egui
  rewrite.

## Out of scope (this epic)

- Accessibility / accesskit wiring (deferred, lock 11).
- A token-crate design system or raw-literal lint gate (explicitly dropped).
- Hyperscale / >3-LH / multi-tenant (§8 envelope unchanged).
- Mesh substrate redesign (§1 unchanged; only additive desktop-state topics).
- Immutable/bootc imaging (packaging stays RPM + kickstart spin, lock 13).

## Open item — resolved

- **Codename for the 12.0 series.** **Confirmed "Quasar"** (operator, 2026-06-30).
  The package/repo id stays `magic-mesh`. Per operator go-ahead, the E12
  governance rewrite was applied to `AI_GOVERNANCE.md` and this design doc landed
  at plan-lock time, so **E12-0 is complete**; E12-1…E12-12 remain the execution
  backlog.
