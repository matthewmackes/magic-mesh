---
name: polish
description: >-
  Iteratively beautify, enhance, evolve, complete and drive the MCNF egui/wgpu
  surfaces to a refined Quasar-dark (Carbon-inspired) finish — fanned out across
  the Xen build farm, one disjoint surface×axis unit per worker. TRIGGER when
  the operator says "polish the GUI", "beautify the app", "make the UI nicer",
  "refine the interface", "drive the GUI to done", or "fan out GUI improvements
  across the farm". Each improvement lands as glue over the shared
  `mde-egui` Style/Motion/Fonts modules, passes the style-leak grep, builds +
  tests green, and commits (never pushes). NOT for finding dead code (/audit),
  a single scoped edit (just do it), or a release cut (/release).
---

# polish — farm-dispatched iterative GUI refinement (MCNF, E12 egui era)

The aesthetic/UX counterpart to `/ship`. Where `/ship` drains the *general*
worklist, `polish` drives a **GUI-only** refinement loop over the E12 egui
surfaces: it surveys every panel against the **Quasar dark** design language
(below), turns the gaps into a **file-disjoint backlog** under the existing
`E12-POLISH` epic, and saturates the farm with one worker per surface×axis unit
until each surface is §7-complete and design-true. It is beautify → enhance →
evolve → complete → drive, in that order, on a loop.

The rulebook is the root **`AI_GOVERNANCE.md`** (this repo has **no `CLAUDE.md`**).
Load-bearing sections for this skill: **§4** (egui-native look — the single source
of look is the shared `Style`/`Visuals` module in `crates/shared/mde-egui`; the
Carbon-token crate and its lint gates are retired), **§5** (one egui shell owning
the DRM seat; every surface is a panel inside it), **§6** (layered tiers — new UI
code is glue, checked by `lint-layered-tiers.sh`), **§7** (Definition of Done —
runtime-reachable, no stubs/mockups), and **§10/§10.0** (work the farm — fan out,
saturate, never grind solo or serialize decomposable work). Re-read it at the
start of every run; it changes.

> **The visual gate stays lifted (operator, 2026-06-11), with one upgrade
> (operator survey, 2026-07-03):** a GUI change is *done* when it builds, tests
> green, passes the style-leak grep, and renders through the shared `mde-egui`
> `Style`. **Headless screenshots** (below) are the eyes-on tool — best-effort,
> *never* a blocker. Do not hold a polish unit `[>]` for a visual check; do not
> flinch from a GUI unit because its feedback is fuzzy (`/no-flinch`).

## The surfaces (E12 — everything is egui; the iced/Cosmic stack is gone)

All rendering goes through **`crates/shared/mde-egui`** (the eframe/wgpu harness:
bare DRM-seat runner + windowed fallback) and its modules — `style` (the
`Style`/`Visuals` single source + `Density`), `motion` (the shared
duration/easing table), `fonts`, `widgets`, `toast`, `gestures`/`touch`,
`formfactor`, `video_plane`. The surface crates live in `crates/desktop/`:

```sh
cargo run -p mde-shell-egui     # THE shell (chrome bar → Workbench) — the long pole
cargo run -p mde-panel-egui     # panel chrome
cargo run -p mde-files-egui     # files panel
cargo run -p mde-music-egui     # music panel (+ mde-media-egui / mde-media-core)
cargo run -p mde-voice-egui     # voice/SIP panel
cargo run -p mde-editor-egui    # editor panel
cargo run -p mde-term-egui      # terminal panel
cargo run -p mde-bookmarks-egui # bookmarks panel
cargo run -p mde-mesh-view      # mesh map view
```

Plus the VDI viewers (`mde-vdi-rdp` / `mde-vdi-spice` / `mde-vdi-vnc`,
egui-texture clients), `mde-web-preview-client`, and `mde-seat`. **Do not polish
retired iced/libcosmic code** — if you find any still referenced, that is a
rescue unit (delete or rehome), not a polish target.

## The design language (operator-locked, 20-Q survey 2026-07-03)

**Quasar dark** — Carbon-inspired, not Carbon-strict. Carbon's sensibilities
(the gray ramp, the 8px rhythm, restraint, density) are the foundation; the
rendering idiom is egui/wgpu-native. Where these locks are silent, workers
follow the craft standards in **`CRAFT.md`** (this folder) — geometry
discipline, window/menu construction, the five interaction states, and the
per-unit review pass. The locks always win over CRAFT.md. The locks:

1. **One theme: dark only.** The Gray-100-derived Quasar dark palette in
   `mde_egui::style`. No light theme, no theme switcher.
2. **Soft-Carbon depth.** Gently rounded corners (4–8px tiers), layered soft
   shadows, explicit elevation tiers. Not flat-and-sharp Carbon, not
   floaty-macOS. **Translucency is subtle only**: slight alpha + dim on
   overlays/scrims — **no true gaussian-blur pass**.
3. **Mono-first typography.** Monospace for headings, nav, data, metrics, IDs,
   code; a humanist sans only for long-form prose. The primary mono is
   **IBM Plex Mono**, embedded in `mde-egui` (deterministic on the immutable
   image). Migrating `fonts.rs` off the current Fira Code default (and deciding
   the prose sans + the fallback chain) is itself a polish unit.
4. **macOS-level motion — all of it, all shared.** Spring physics for
   panel/sheet transitions, inertial scrolling with rubber-band overscroll,
   micro-interactions (hover lift, press scale, focus glow, animated toggles),
   and choreographed transitions (staggered list entrances, cross-fades, the
   chrome-bar→Workbench hero expansion). Every primitive lives in
   `mde_egui::motion` (extending the `Motion` FAST/BASE/SLOW table); a surface
   crate NEVER hand-rolls a tween or a literal duration.
5. **Full a11y is a polish axis** (operator override of the §4 deferral as a
   *target*, though it is still not a §7 gate): visible 2px focus ring,
   complete keyboard reachability, contrast held on every pair, hit targets,
   accesskit groundwork where egui exposes it.
6. **Carbon icon set ships with the platform.** IBM's Carbon icons (Apache-2.0)
   embedded and exposed via `mde-egui`; no inline glyph soup, no hard-coded
   asset paths.
7. **Auto DPI + density modes.** Honor per-display `pixels_per_point`; extend
   the existing `Density` (Mouse/Touch) toward compact/comfortable presets.
   Density scales **spacing only, never component dimensions** (UX-24).
8. **Performance is NOT a gated axis.** No frame-time budget gate. Workers may
   *observe* frame time while working motion units; a hitch is a bug to file,
   not a polish dimension.
9. **Component kit: evaluate before building.** Phase 0 inventories what
   `mde_egui::widgets`/`toast` and the surfaces already have before any new
   shared component (data table, command palette, cards, empty states) is
   proposed. Consolidation of near-duplicates beats new construction.

## The single hard rule (§4, upgraded with the style-leak grep)

**The shared `Style`/`Motion`/`Fonts` in `mde-egui` are the only source of
look.** A polish unit that "beautifies" by minting a `Color32::from_rgb(...)`
or a literal animation duration in a surface crate is a regression, not an
improvement. If a needed value is genuinely missing, add it to `mde-egui`
*with a backing test*, then consume it.

The mechanical gate (run from the repo root; **zero hits required** in
`crates/desktop`). Pixel-format conversion and ANSI palettes are **data, not
look** — the VDI decoders and the terminal colour table are excluded:

```sh
# style-leak grep — colours and bespoke durations minted outside mde-egui
grep -rnE 'Color32::from_(rgb|rgba|gray|black_alpha|white_alpha)' \
  crates/desktop --include='*.rs' \
  | grep -vE 'mde-vdi-(rdp|spice|vnc)/|mde-term-egui/src/(palette|presets)\.rs'
grep -rnE 'animate_bool_with_time\([^)]*[0-9]\.[0-9]' \
  crates/desktop --include='*.rs'
```

(Promote to `install-helpers/lint-style-leaks.sh` when convenient; until then
the inline grep IS the gate. The retired Carbon/motion lints stay retired.
Baseline at adoption, 2026-07-03: **4 hits** — `mde-shell-egui/src/splash.rs`
×1, `mde-term-egui/src/widget.rs` ×3 — each is a ready-made first rescue unit.)

## Quality axes (the polish dimensions — expanded for Rust/wgpu)

Each is an independent unit of work — one worker owns one axis on one surface so
units stay file-disjoint and the farm parallelizes cleanly:

1. **Spacing & rhythm** — the 8px grid via shared `Style` spacing; density
   presets scale spacing tokens only (UX-24). No ad-hoc gaps.
2. **Typography** — the mono-first stack (lock 3); correct tier per role, no
   off-scale sizes, prose in the sans.
3. **Colour & contrast** — Quasar dark palette values only; contrast holds on
   every text/background pair.
4. **Depth & materials** — the soft-Carbon radii/shadow/elevation tiers +
   subtle-translucency scrims (lock 2), applied consistently.
5. **Motion** — the macOS-level behaviors (lock 4), consumed from
   `mde_egui::motion` only; reduce-motion aware.
6. **Focus & a11y** — 2px focus ring, keyboard reachability, hit targets,
   accesskit groundwork (lock 5).
7. **Empty / loading / error states** — skeletons and designed empty states
   instead of blank panels or frozen spinners. No `demo_data`/placeholder
   passing as content (that's a §7 mockup).
8. **Component reuse & consolidation** — collapse one-off widgets onto the
   `mde-egui` kit (evaluate-first, lock 9). New code is glue (§6).
9. **Iconography & brand** — the embedded Carbon icon set (lock 6) via
   `mde-egui`; no inline glyphs, no hard-coded paths.
10. **Layout, responsiveness & DPI** — sane reflow across window sizes and
    formfactors; auto `pixels_per_point`; chrome consistency across panels;
    crisp rendering at fractional scales (no blurry hairlines).
11. **Render quality (wgpu)** — 1px strokes land on pixel boundaries, textures
    aren't stretched, `video_plane` composition is clean. Frame time is
    observed here, never gated (lock 8).
12. **Completeness / "drive to done"** — a panel that renders but whose state
    never updates is half-built (§7). Wire the live data, finish the
    interaction, remove the "coming soon".

## Headless screenshots (the eyes-on tool)

`/preview` predates E12 and targets the retired stack; the E12 replacement is
an **offscreen capture path**: render a surface via the eframe/wgpu windowed
runner (or a headless wgpu target) to a PNG, then `Read` the PNG. If the
harness lacks the capture hook, adding one small `mde-egui` capture entry point
is a high-value early polish unit. Screenshots are **best-effort evidence,
never a blocker** — a unit ships on green gates alone.

## The farm (exact topology — know it cold)

Four Xen build VMs, all **Fedora 42**, user `mm`, key
`/root/.ssh/mackes_mesh_ed25519`, **shared sccache** (`RUSTC_WRAPPER=sccache`):

| Host | VM | IP | vCPU / RAM | SAFE heavy slots |
|---|---|---|---|---|
| **XEN-BIGBOY** | `mcnf-build-52` | `172.20.0.130` | 12 / 20 GB | **3** |
| KVM-XCP1 | `mcnf-build-51` | `172.20.0.90` | 4 / 16 GB | **2** |
| XEN-HOME-SERVICES | `mcnf-build-50` | `172.20.0.50` | 4 / 16 GB | **2** |
| XEN-194 | `mcnf-build-53` | `172.20.0.170` | 4 / 16 GB | **2** |

> **Canonical roster + verified table:** `install-helpers/farm-topology.sh` is the
> single source of truth (4 dom0s / 4 build VMs / 9 slots). Run
> `./install-helpers/farm-topology.sh table` at the start of every run to chart a
> **verified** utilization table — it probes all 4 nodes and fails if one is
> missing. Never chart from memory or hardcode the node list (this table once
> silently missed the 4th dom0 XEN-194/.170).

> ⚠️ **VM names are legacy and do NOT equal the IP octet** (`docs/BUILD-ENVIRONMENT.md`):
> `mcnf-build-51`=**.90**, `mcnf-build-52`=**.130**, `mcnf-build-53`=**.170**. The
> real farm is the **4 build VMs .50 / .90 / .130 / .170**; probing `.51`/`.52`
> gives "No route to host" (not a node-down alarm).

**Total = 9 concurrent heavy build slots, spread 2 + 2 + 3 + 2** (.50/.90/.170 at
2, BigBoy .130 at 3). The GUI crates ride the egui/eframe/wgpu stack — a cold
GUI compile is a *heavy* slot. That slowness is exactly why this work must go to
the farm and never grind locally (`/no-flinch` rule 4: fix the loop, don't avoid it).

### The hard cap (the load-44 lesson — NON-NEGOTIABLE)
**≤3 heavy builds per node. NEVER more.** 6 concurrent heavies on BIGBOY → load
44, disk full, lost work (`AI_GOVERNANCE.md §10`). Full utilization = fill *to* the
cap, *spread* — not pile onto BIGBOY. 4-vCPU nodes cap at 2; the 12-vCPU BIGBOY at 3.

### BigBoy takes the longest / most-complex build (standing rule, operator 2026-06-30)
Complementary to the spread cap: the **single heaviest job always goes to BIGBOY**
(`.130`, 12 vCPU) — a full `cargo --workspace` build/test/clippy, the biggest egui
crates (**`mde-shell-egui`** above all), a cold wgpu compile, the RPM release.
The 4-vCPU nodes (`.50`/`.90`/`.170`) take the shorter/simpler jobs (small
single crates, per-crate tests/clippy). Spread the *count* to honor caps; route the
*long pole* to BigBoy first — never leave BigBoy on a trivial build while a small
node grinds the workspace.

### Slot mechanics (so concurrent builds don't clobber)
`install-helpers/xcp-build.sh` derives `REMOTE_DIR="magic-mesh${MCNF_BUILD_SLOT:+-$MCNF_BUILD_SLOT}"`.
Every concurrent build needs a **unique slot name on its host** (its own `target/`);
two builds sharing one (host, slot) clobber via rsync `--delete`. A slot-assigning
workflow uses numeric slots `1/2/3` indexed over `[.130/1, .130/2, .130/3, .50/1,
.50/2, .90/1, .90/2, .170/1, .170/2]` (9 slots); ad-hoc/second-campaign builds use **named** slots
(`polishA`, `polish-shell`) within the *remaining* per-node headroom. **Two
slot-assigning coordinators at once is FORBIDDEN** — they index the same array and
clobber.

### Rearm — never drain-and-wait
The instant a slot's build finishes, refill it with the next queued unit. Do NOT
batch-launch N, await all N, then launch N more — that idles every fast slot for
the slowest build. Use `pipeline()` (no barrier between stages) or per-slot
completion handlers, not `parallel(); await; parallel()`. Detach long builds
(`run_in_background: true` / `nohup … &`) so they aren't sandbox-killed (exit
143/144), and monitor a log file.

### The coordinator helpers (don't re-derive by hand)
- **`install-helpers/drain-coordinator.sh plan [N]`** — one tick: pre-flight
  `disk-watchdog.sh` → free-slot compute over the REAL topology (`.50/.90/.130/.170`,
  caps 2/2/3/2) → next N open, unblocked unit ids. `… slots` / `… next N` /
  `… preflight` run the pieces alone.
- **`install-helpers/assert-own-worktree.sh`** — every isolated worker runs this as
  STEP-0; it REFUSES the shared/primary checkout so a worker can't reset the
  coordinator's tree. Each worker prompt MUST open with it.
- **`install-helpers/park-blocker.sh <ID> "<reason>"`** — when a unit hits a
  blocker you can't clear (a missing shared primitive, live infra), PARK it:
  flips to `[!]`, surfaces it in `docs/NEEDS-OPERATOR.md`, exits 0 so the loop
  continues. Never stall a whole tick on one item.
- **Monitor:** `df -h /home` stays **< 90%**, load **< ~2× vCPU**; GC stale slot
  dirs (`farm-slot-gc.sh`, or `rm -rf ~/magic-mesh-<stale>` for finished workers).

## The loop

### Phase 0 — Refresh + survey (every run, before dispatch)
1. **Re-read `AI_GOVERNANCE.md`** (§4/§5/§6/§7/§10) and skim the relevant
   `docs/design/*.md` (`quasar-branding.md`, `quasar-vdi-desktop.md`,
   `mesh-shell.md`, `kiron-toast-pattern.md`, the per-surface docs). Never
   polish from a stale memory of the locks.
2. **Inventory the kit first (lock 9).** Read `mde_egui::{style,motion,fonts,
   widgets,toast}` and list what exists vs. what surfaces hand-roll. Every
   near-duplicate widget across two surfaces is a consolidation unit.
3. **Survey each surface against the 12 axes.** Build the GUI workspace once on
   the farm, then inspect each panel (headless screenshot where the capture
   path exists, code-read where it doesn't). Note every gap as a candidate
   unit. Back the visual read with the static ground truth:
   `cargo test -p mde-egui`.
4. **Rescue first (cheap, high-value).** Catch the recurring failure modes
   before adding polish: a panel whose state never updates,
   `demo_data`/placeholder/"coming soon" strings, a `pub mod` with no caller,
   surviving iced/libcosmic/`mde-theme` references, style-leak grep hits,
   `lint-layered-tiers.sh` violations. Each hit is a unit.

### Phase 1 — Backlog (the durable record)
Lift every gap + rescue into **`docs/WORKLIST.md`** under the existing
**`### E12-POLISH`** epic (the single durable tracker). Use the `/plan`
user-story schema, one task per **surface×axis** so units stay file-disjoint:

```
- [ ] **POLISH-<surface>-<axis>: <surface> — <axis> to Quasar dark**
  **As** a mesh operator,
  **I want** <surface>'s <axis> to match the Quasar dark design language,
  **so that** <outcome>.
  **Acceptance** (each runtime-observable):
    - [ ] reads only mde-egui Style/Motion/Fonts for <axis> (style-leak grep clean)
    - [ ] renders correctly in the Quasar dark theme at 1.0 and a fractional scale
    - [ ] <specific observable improvement>
```

Status legend (from `/plan`): `[ ]` open, `[>]` in progress (carry `session=<id>`),
`[✓]` done, `[!]` blocked. No silent deferrals.

### Phase 2..N — Drain across the farm
Per tick: `drain-coordinator.sh plan` → dispatch **one disjoint unit per free
slot**, spread to honor the per-node cap, each as a farm-only isolated worker →
on each completion, integrate (cherry-pick/merge) + reclaim the worktree +
**rearm** with the next unit (never batch-wait) → `park-blocker.sh` anything
blocked and move on. Disjointness keeps the workers from colliding: assign no two
in-flight workers the same surface crate, and treat `mde-egui` itself as ONE
surface (shared-kit units serialize; surface units parallelize).

#### Worker prompt template (one GUI-polish unit)
> **STEP-0:** run `./install-helpers/assert-own-worktree.sh`; abort if it exits
> non-zero.
> **Task:** POLISH-`<surface>`-`<axis>`. Improve **only** the `<axis>` of the
> `<surface>` surface, in its crate only. Read the relevant `docs/design/*.md` +
> `AI_GOVERNANCE.md §4` + the design-language locks in this skill.
> **Rules:** every colour, metric, radius, shadow, duration and easing you draw
> reads `mde_egui::{Style,Motion,fonts}` — no `Color32::from_*`, no literal
> duration, in the surface crate. If a needed value/primitive is genuinely
> missing, add it to `mde-egui` *with a backing test* and consume it. New code
> is glue over the existing crates (§6 tiers). Do not cross into another
> surface's crate.
> **Build on the farm** with a unique `MCNF_BUILD_SLOT` (`install-helpers/xcp-build.sh`).
> **Gate (all green before commit):** `cargo build -p <crate>` (or `--workspace`),
> `cargo test` (and `cargo test -p mde-egui` for any shared-kit change),
> `cargo clippy --all-targets`, `cargo fmt --all`, the **style-leak grep**
> (zero hits in `crates/desktop`), `./install-helpers/lint-layered-tiers.sh`.
> A headless screenshot is optional evidence, never a blocker.
> **Commit** named pathspecs with a why-not-what message + the repo's
> `Co-Authored-By` trailer (see `/ship`). Flip the unit `[✓]`. **Do NOT push.**

### Iterate
After a wave, re-survey the touched surfaces (Phase 0 step 3) — refinement
exposes the next gap. Keep waving until every surface×axis unit is `[✓]` and the
gates are clean. Many surfaces fully design-true > many half-polished
foundations (`/no-flinch` rule 5: finish over pile).

## Gating a polish change (Definition of Done, §7)
A unit is done only when, from the repo root:
- `cargo build --workspace` (or `-p <crate>`) clean.
- `cargo test` green (+ `cargo test -p mde-egui` for any shared-kit edit — the
  design-language ground truth).
- `cargo clippy --all-targets` + `cargo fmt --all` clean.
- **Style-leak grep clean** (zero hits in `crates/desktop`) +
  `lint-layered-tiers.sh` clean (§6).
- The surface still **launches and updates** (`timeout 3 cargo run -p <crate>` —
  no panic, real state, not `demo_data`).
SOFT-ESCAPE if the same gate fails 3× — park it (`park-blocker.sh`) and keep the
loop moving rather than grinding one unit.

## Stop conditions
Backlog empty (only gated/parked units remain) · a push/release moment · a
destructive op · a product-direction change · two consecutive unexplained gate
failures · ≥10 rescues at once. On stop: a short factual summary + what's left.
**Pushing stays operator-gated** (`git push origin master`, single remote, only on
explicit go-ahead); the RPM cut is always `/release`.

## NOT this skill
Single obvious GUI edit → just do it. Find dead/mock/stub UI with a written
report → `/audit`. Design / survey / author the backlog before code → `/plan`.
Drain the *general* (non-GUI) worklist → `/ship`. Release cut → `/release`.
Catch yourself routing around the slow/fuzzy GUI work → `/no-flinch`.
