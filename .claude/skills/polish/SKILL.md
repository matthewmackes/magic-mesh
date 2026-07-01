---
name: polish
description: >-
  Iteratively beautify, enhance, evolve, complete and drive the MCNF
  iced/Cosmic GUIs to a higher Carbon-fidelity finish — fanned out across the
  Xen build farm, one disjoint surface×dimension unit per worker. TRIGGER when
  the operator says "polish the GUI", "beautify the app", "make the UI nicer",
  "drive the GUI to done", or "fan out GUI improvements across the farm". Each
  improvement lands as glue over `mde-theme` tokens (§4), builds + tests green,
  and commits (never pushes). NOT for finding dead code (/audit), a single
  scoped edit (just do it), or a release cut (/release).
---

# polish — farm-dispatched iterative GUI evolution (MCNF)

The aesthetic/UX counterpart to `/ship`. Where `/ship` drains the *general*
worklist, `polish` drives a **GUI-only** improvement loop: it surveys the iced
0.14 client areas against the IBM-Carbon reference, turns the gaps into a
**file-disjoint backlog**, and saturates the farm with one worker per
surface×dimension unit until each surface is §7-complete and Carbon-true. It is
beautify → enhance → evolve → complete → drive, in that order, on a loop.

The rulebook is the root **`AI_GOVERNANCE.md`** (this repo has **no `CLAUDE.md`**).
Load-bearing sections for this skill: **§4** (Carbon look — tokens single-sourced
in `crates/shared/mde-theme`, lint-gated), **§6** (mesh/desktop boundary — Cosmic
owns the desktop; MCNF draws only its client areas), **§7** (Definition of Done —
runtime-reachable, no stubs/mockups), and **§10/§10.0** (work the farm — fan out,
saturate, never grind solo or serialize decomposable work). Re-read it at the
start of every run; it changes.

> **The visual gate is lifted (2026-06-11, operator).** A GUI change is *done*
> when it builds, tests green, and renders through the `mde-theme` Carbon tokens.
> `/preview` is optional/best-effort — never a blocker. Do not hold a polish unit
> `[>]` solely for an on-Cosmic visual check; do not flinch from a GUI unit
> because its feedback is fuzzy (`/no-flinch`).

## The surfaces (each is its own binary)

```sh
cargo run -p mde-workbench    # the Cosmic control surface (fleet, devices, mesh health)
cargo run -p mde-files        # the file manager
cargo run -p mde-voice-hud    # voice/SIP HUD            (mde-voice-config = its config)
cargo run -p mde-music        # the music player          (mde-musicd = its daemon)
cargo run -p magic-fleet      # the Automation Mesh node engine
```

Plus the install-time surfaces `mde-role-chooser` + `mde-cosmic-applet`. The
shared look stack is `crates/shared/`: **`mde-theme`** (the Carbon token source —
`color`/`palette`/`carbon`, `spacing`, `typography`, `radii`, `shadows`,
`motion`/`animation`/`frame_timer`, `density`, `accessibility`, `skeleton`,
`feedback`, `components/{object_card,empty_state}`), **`mde-card`**, and
**`mde-disclaimer`**. `salvage/from-mde-binary/` holds two not-yet-rehomed
surfaces (`birthright`, `mesh_status`).

## The single hard rule (§4)

**No raw colour, no scattered metric, ever, outside `crates/shared/mde-theme`.**
Every hue, size, space, radius, shadow, duration and easing a surface draws must
*read a `mde-theme` token*. A polish unit that "beautifies" by minting a
`Color::from_rgb(...)` or a literal `16.0` in a surface crate is a §4 violation
and will fail the gate — it is not an improvement, it is a regression. Beautifying
means: pick (or, if genuinely missing, *add to `mde-theme` with a test*) the right
token, then make the surface consume it.

## Quality dimensions (the polish axes)

Each is an independent unit of work — one worker owns one axis on one surface so
units stay file-disjoint and the farm parallelizes cleanly:

1. **Spacing & rhythm** — the 8px / 12-step modular scale (`spacing`). No ad-hoc
   gaps; consistent gutters, padding, list rhythm. `density` scales spacing tokens
   only, never component dimensions (UX-24).
2. **Typography** — the Carbon type scale + font stack (`typography`). Correct
   weight/size/line-height per tier; no off-scale sizes.
3. **Colour & contrast** — palette tokens only; verify WCAG contrast via
   `accessibility`. Gray 100 (default dark) / Gray 90 / Gray 10 must all hold.
4. **Motion** — durations/easings/staggers from `motion`+`animation`+`frame_timer`;
   reduce-motion aware. Bespoke tweens are forbidden (gated by `lint-motion.sh`).
5. **Focus & a11y** — the 2px Carbon focus ring, keyboard reachability, hit
   targets, contrast. A surface you can't drive from the keyboard isn't finished.
6. **Empty / loading / error states** — the `empty_state` + `skeleton` primitives
   instead of a blank panel or a frozen spinner. No `demo_data`/placeholder
   passing as content (that's a §7 mockup, not a polished state).
7. **Component reuse & consolidation** — collapse one-off widgets onto
   `mde-card` / `mde-theme::components` / `object_card`. New code is glue, not
   reimplementation (§6). Fewer bespoke widgets = more consistency for free.
8. **Iconography & brand** — `mde-theme::icons` + the `Brand` loader; no inline
   glyph soup, no hard-coded asset paths.
9. **Layout & responsiveness** — sane reflow across window sizes; nav grouping;
   `panel_chrome`/`header`/`sidebar` consistency across surfaces.
10. **Completeness / "drive to done"** — a panel that renders but whose state
    never updates is half-built (§7). Wire the live data, finish the interaction,
    remove the "coming soon".

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
2, BigBoy .130 at 3). The GUI crates ride the egui/wgpu (E12) or libcosmic/iced
(legacy) stack — a cold GUI compile is a
*heavy* slot (~1 hr cold). That slowness is exactly why this work must go to the
farm and never grind locally (`/no-flinch` rule 4: fix the loop, don't avoid it).

### The hard cap (the load-44 lesson — NON-NEGOTIABLE)
**≤3 heavy builds per node. NEVER more.** 6 concurrent heavies on BIGBOY → load
44, disk full, lost work (`AI_GOVERNANCE.md §10`). Full utilization = fill *to* the
cap, *spread* — not pile onto BIGBOY. 4-vCPU nodes cap at 2; the 12-vCPU BIGBOY at 3.

### BigBoy takes the longest / most-complex build (standing rule, operator 2026-06-30)
Complementary to the spread cap: the **single heaviest job always goes to BIGBOY**
(`.130`, 12 vCPU) — a full `cargo --workspace` build/test/clippy, the biggest egui
crates (`mde-shell-egui` / `mde-workbench`), a cold cosmic/iced/wgpu compile, the RPM
release. The 4-vCPU nodes (`.50`/`.90`/`.170`) take the shorter/simpler jobs (small
single crates, per-crate tests/clippy). Spread the *count* to honor caps; route the
*long pole* to BigBoy first — never leave BigBoy on a trivial build while a small
node grinds the workspace.

### Slot mechanics (so concurrent builds don't clobber)
`install-helpers/xcp-build.sh` derives `REMOTE_DIR="magic-mesh${MCNF_BUILD_SLOT:+-$MCNF_BUILD_SLOT}"`.
Every concurrent build needs a **unique slot name on its host** (its own `target/`);
two builds sharing one (host, slot) clobber via rsync `--delete`. A slot-assigning
workflow uses numeric slots `1/2/3` indexed over `[.130/1, .130/2, .130/3, .50/1,
.50/2, .90/1, .90/2, .170/1, .170/2]` (9 slots); ad-hoc/second-campaign builds use **named** slots
(`polishA`, `polish-workbench`) within the *remaining* per-node headroom. **Two
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
  blocker you can't clear (a missing token, live infra), PARK it: flips to `[!]`,
  surfaces it in `docs/NEEDS-OPERATOR.md`, exits 0 so the loop continues. Never
  stall a whole tick on one item.
- **Monitor:** `df -h /home` stays **< 90%**, load **< ~2× vCPU**; GC stale slot
  dirs (`farm-slot-gc.sh`, or `rm -rf ~/magic-mesh-<stale>` for finished workers).

## The loop

### Phase 0 — Refresh + survey (every run, before dispatch)
1. **Re-read `AI_GOVERNANCE.md`** (§4/§6/§7/§10) and skim the relevant
   `docs/design/*.md` (e.g. `motion-system.md`, `branding.md`,
   `workbench-nav-grouping.md`, the per-surface docs). Never polish from a stale
   memory of the locks.
2. **Survey each surface against the 10 axes.** Build the GUI workspace once on
   the farm, then for each surface inspect the client area (`/preview` is the tool
   for this — launch the real binary, or capture + `Read` the PNG headless). Note
   every gap as a candidate unit. Back the visual read with the static token
   ground truth: `cargo test -p mde-theme`.
3. **Rescue first (cheap, high-value).** Catch the project's recurring failure
   mode before adding polish: a surface that renders but whose state never updates,
   `demo_data`/placeholder/"coming soon" strings, a `pub mod` with no caller, a
   raw-hex/scattered-metric leak. Run the GUI gates read-only:
   `lint-carbon-tokens.sh`, `lint-motion.sh`, `lint-mesh-boundary.sh`,
   `lint-no-cratesio-iced.sh`, `lint-libcosmic-rev.sh`. Each hit is a unit.

### Phase 1 — Backlog (the durable record)
Lift every gap + rescue into **`docs/WORKLIST.md`** under a `### GUI-POLISH`
epic (the single durable tracker; the `GUI` / `BRAND` / `MESHMAP` / `MUSIC*` /
`NOTIFY-UI` epics already there are valid homes too). Use the `/plan` user-story
schema, one task per **surface×axis** so units stay file-disjoint:

```
- [ ] **POLISH-<surface>-<axis>: <surface> — <axis> to Carbon**
  **As** a mesh operator,
  **I want** <surface>'s <axis> to match the Carbon reference,
  **so that** <outcome>.
  **Acceptance** (each runtime-observable):
    - [ ] reads only `mde-theme` tokens for <axis> (lint-clean)
    - [ ] renders correctly in Gray 100 / 90 / 10
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
in-flight workers the same surface crate.

#### Worker prompt template (one GUI-polish unit)
> **STEP-0:** run `./install-helpers/assert-own-worktree.sh`; abort if it exits
> non-zero.
> **Task:** POLISH-`<surface>`-`<axis>`. Improve **only** the `<axis>` of the
> `<surface>` surface, in its crate only. Read the relevant `docs/design/*.md` +
> `AI_GOVERNANCE.md §4`.
> **Rules:** every value you draw reads a `crates/shared/mde-theme` token — no raw
> `Color::from_rgb*`, no literal metric, in the surface crate (§4). If a needed
> token is genuinely missing, add it to `mde-theme` *with a backing test* and
> consume it. New code is glue over the existing crates, not reimplementation (§6).
> Do not cross into another surface's crate; do not touch desktop-shell concerns
> (Cosmic owns those, §6).
> **Build on the farm** with a unique `MCNF_BUILD_SLOT` (`install-helpers/xcp-build.sh`).
> **Gate (all green before commit):** `cargo build -p <crate>` (or `--workspace`),
> `cargo test` (and `cargo test -p mde-theme` for any token change),
> `cargo clippy --all-targets`, `cargo fmt --all`, `./install-helpers/lint-carbon-tokens.sh`,
> `./install-helpers/lint-motion.sh`, `./install-helpers/lint-mesh-boundary.sh`.
> `/preview` is optional/best-effort, never a blocker.
> **Commit** named pathspecs with a why-not-what message + the repo's
> `Co-Authored-By` trailer (see `/ship`). Flip the unit `[✓]`. **Do NOT push.**

### Iterate
After a wave, re-survey the touched surfaces (Phase 0 step 2) — beautify exposes
the next gap. Keep waving until every surface×axis unit is `[✓]` and the GUI gates
are clean. Many surfaces fully Carbon-true > many half-polished foundations
(`/no-flinch` rule 5: finish over pile).

## Gating a polish change (Definition of Done, §7)
A unit is done only when, from the repo root:
- `cargo build --workspace` (or `-p <crate>`) clean — a full build needs
  `gtk3-devel` + `alsa-lib-devel`; `.cargo/config.toml` sets
  `CMAKE_POLICY_VERSION_MINIMUM=3.5` for the vendored Opus tree.
- `cargo test` green (+ `cargo test -p mde-theme` for any token edit — the Carbon
  ground truth).
- `cargo clippy --all-targets` + `cargo fmt --all` clean.
- **GUI gates clean:** `lint-carbon-tokens.sh` (§4 single-source),
  `lint-motion.sh` (no bespoke animation), `lint-mesh-boundary.sh` (§6),
  `lint-no-cratesio-iced.sh` + `lint-libcosmic-rev.sh` (the iced-fork / libcosmic
  pin guards).
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
report → `/audit`. Verify a render actually looks right → `/preview`. Design /
survey / author the backlog before code → `/plan`. Drain the *general* (non-GUI)
worklist → `/ship`. Release cut → `/release`. Catch yourself routing around the
slow/fuzzy GUI work → `/no-flinch`.
