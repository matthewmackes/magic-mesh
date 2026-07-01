---
name: ship
description: >-
  Autonomously drain the MCNF worklist: a rescue pass to catch
  dead/mock code, then implement open tasks fully (no stubs), building +
  verifying each, committing as you go. TRIGGER when the user says "ship it",
  "execute", "continue", "drain the worklist", or "work through the backlog" for
  this Rust mesh workspace. Do NOT use for a single scoped edit (just do it) or
  anything needing a release cut (use /release).
---

# ship — autonomous worklist drain (MCNF)

Drains the worklist to empty under operator direction. Heads-down: the commit body
is the record, one short note per phase boundary, no marketing copy. The rulebook is
the root `AI_GOVERNANCE.md` (this repo has **no `CLAUDE.md`**); the load-bearing
sections are §4 (Carbon look), §6 (the mesh boundary), and §7 (Definition of Done).

> **Worklist may not exist yet.** There is no `docs/` directory in the repo today —
> `docs/WORKLIST.md` is the intended single tracker, created when execution begins.
> If it is absent, the durable plan is `AI_GOVERNANCE.md` (the E11 "MCNF"
> pivot identity + architectural locks); pull the next actionable items from there
> and from any `docs/design/*.md` once they exist, and create the worklist as the
> durable record before draining.

## Phase 0a — Refresh governance (MANDATORY, every run, before anything else)

**Re-read `AI_GOVERNANCE.md` into context at the start of every `/ship` run** — it is
the rulebook and it changes; never drain from a stale memory of it. *(operator,
2026-06-22.)* `Read` the whole file. Pay special attention to the load-bearing
sections: **§10.0 (MANDATE: work the farm — offload builds + fan out concurrent
subagents across the farm (`.50/.90/.130/.170`), never grind solo or serialize decomposable work)**,
§4 (Carbon look), §6 (the mesh boundary), and §7 (Definition of Done). If you catch
yourself building locally/sequentially when the work could go to the farm in
parallel, that's a §10.0 violation — fix it before continuing.

## Phase 0b — FULL XEN-HOST UTILIZATION (standing MANDATE, operator 2026-06-23)

> **The directive:** *"Achieve full utilization of the XEN Hosts."* Every Xen build
> slot must be doing productive work **whenever there is anything buildable** (a
> worklist unit, a gate run, an RPM cut, a test suite). A Xen host sitting idle
> while buildable work exists is a violation of the same class as grinding solo
> (§10.0). This is not "use the farm when convenient" — it is **saturate the farm,
> keep it saturated, and rearm the instant a slot frees.** Canonical farm detail
> lives in `docs/BUILD-ENVIRONMENT.md` + `AI_GOVERNANCE.md §10`; this is the
> operating procedure.

### The farm (exact topology — know it cold)
Four Xen build VMs, all **Fedora 42** (an F42-built RPM installs on F43+F44 —
older-glibc forward-compat), user `mm`, key `/root/.ssh/mackes_mesh_ed25519`,
**shared sccache** (`RUSTC_WRAPPER=sccache`):

| Host | VM | IP | vCPU / RAM | SAFE heavy slots |
|---|---|---|---|---|
| **XEN-BIGBOY** | `mcnf-build-52` | `172.20.0.130` | 12 / 20 GB | **3** |
| KVM-XCP1 | `mcnf-build-51` | `172.20.0.90` | 4 / 16 GB | **2** |
| XEN-HOME-SERVICES | `mcnf-build-50` | `172.20.0.50` | 4 / 16 GB | **2** |
| XEN-194 | `mcnf-build-53` | `172.20.0.170` | 4 / 16 GB | **2** |

> ⚠️ **VM names are legacy and do NOT equal the IP octet** (`docs/BUILD-ENVIRONMENT.md §3`):
> `mcnf-build-51`=**.90**, `mcnf-build-52`=**.130**, `mcnf-build-53`=**.170**. The real
> farm is the **4 build VMs .50 / .90 / .130 / .170** across **4 dom0s** (the 4th is
> XEN-194 / `mcnf-build-53` / `.170`); probing `.51`/`.52` gives "No route to host" (not
> a false "node down"). **`install-helpers/farm-topology.sh` is the single canonical
> roster** — `./install-helpers/farm-topology.sh table` prints a VERIFIED utilization
> table (probes all 4, fails if one is missing); `drain-coordinator.sh slots` sources it.

**Total = 9 concurrent heavy (egui/wgpu · cosmic/iced · mackesd-release) build
slots.** Full utilization = all 9 busy, spread **2 + 2 + 3 + 2** (.50/.90/.170 at 2,
BigBoy .130 at 3).

### The hard cap (the load-44 lesson — NON-NEGOTIABLE)
**≤3 heavy builds per node. NEVER more.** Proven live: 6 concurrent heavy builds on
BIGBOY → load average **44**, disk full, stuck/dud agents whose code had to be
salvaged on the small nodes (`AI_GOVERNANCE.md §10`). "Full utilization" means
filling **to** the cap, *spread*, NOT piling onto BIGBOY. A 4-vCPU node (.50/.90)
caps at **2**; the 12-vCPU BIGBOY (.130) at **3**. Exceeding the cap is the *opposite* of
utilization — it deadlocks the node and you lose the work.

### BigBoy takes the longest / most-complex build (standing rule, operator 2026-06-30)
Complementary to the spread cap: the **single heaviest job always goes to BIGBOY**
(`.130`, 12 vCPU) — a full `cargo --workspace` build/test/clippy, the biggest egui
crates (`mde-shell-egui` / `mde-workbench`), a cold cosmic/iced/wgpu compile, or the
RPM release build. The 4-vCPU nodes (`.50`/`.90`/`.170`) take the shorter/simpler
jobs (small single crates, per-crate tests/clippy). Spread the *count* to honor the
caps; route the *long pole* to BigBoy first — never leave BigBoy idle or on a
trivial build while a small node grinds the workspace.

### Slot mechanics (so concurrent builds don't clobber each other)
`install-helpers/xcp-build.sh` derives the remote dir:
`REMOTE_DIR="magic-mesh${MCNF_BUILD_SLOT:+-$MCNF_BUILD_SLOT}"`. So `MCNF_BUILD_SLOT=2`
→ isolated `~/magic-mesh-2` with its **own** `target/`; `MCNF_BUILD_SLOT=eagledeploy`
→ `~/magic-mesh-eagledeploy`. **Two concurrent builds sharing one (host, slot)
clobber** (rsync `--delete`). Therefore:
- Every concurrent build gets a **unique slot name on its host**.
- A slot-assigning **workflow** uses numeric slots `1/2/3` by index over
  `[.130/1, .130/2, .130/3, .50/1, .50/2, .90/1, .90/2, .170/1, .170/2]` (9 slots).
- Ad-hoc / second-campaign builds use **named** slots (`fixclip`, `eagledeploy`,
  `fillA`) so they never collide with a workflow's numeric dirs — but you MUST still
  count them against that node's ≤3 cap.
- **Two slot-assigning workflows at once is FORBIDDEN** — both index the same numeric
  array and clobber. One coordinator owns the 9 slots at a time; everything else uses
  named slots within the *remaining* per-node headroom.

### Rearm — never drain-and-wait (operator, standing)
*"Always rearm empty slots. DO NOT WAIT until full hosts are idle. Keep the
development cycle continuing."* The instant a slot's build finishes, refill it with
the next queued unit. Do **NOT** batch-launch N, await all N, then launch N more —
that idles every fast slot for the duration of the slowest build. Use `pipeline()`
(no barrier between stages) or per-slot completion handlers, not
`parallel(); await; parallel()`.

### The procedure (reach + HOLD full utilization)
1. **Inventory all four nodes** — simplest is `./install-helpers/farm-topology.sh table`
   (verified; fails if a node is missing). The raw probe:
   ```
   for n in 50 90 130 170; do ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.$n \
     'echo ".'$n' load=$(cut -d" " -f1 /proc/loadavg) rustc=$(pgrep -c rustc) cargo=$(pgrep -c cargo) free=$(df -h --output=avail /home|tail -1|tr -d " ") dirs=$(ls -d ~/magic-mesh-* 2>/dev/null|wc -l)"'; done
   ```
2. **Compute free slots/node** = cap (2/2/3/2) − active heavy builds (a single
   `cargo build` shows as 1 cargo proc + up to N-core rustc fan-out; count distinct
   build dirs/cargo procs, not rustc).
3. **Fill every free slot** with a productive buildable unit (queue below). Distinct
   slot names; spread to honor the per-node cap.
4. **Rearm** on each completion — don't wait for the wave.
5. **Monitor**: `df -h /home` stays **< 90%**, load **< ~2× vCPU**; **clean orphaned
   slot dirs** (`rm -rf ~/magic-mesh-<stale>` for finished agents). Leftover per-agent
   dirs accumulate and fill the disk — the farm hit **96% → 31%** only after
   reclaiming ~72 GB of stale `magic-mesh-1..6`/`-fn5`/`-x`/`-y`.

### The coordinator loop (DRAIN-5/6/7) — concrete tools
The mechanics above are encoded as runnable helpers so a tick never depends on
re-deriving them by hand:
- **`install-helpers/drain-coordinator.sh plan [N]`** — one tick end-to-end:
  pre-flight `disk-watchdog.sh` → free-slot compute over the REAL topology
  (`.50/.90/.130/.170`, caps 2/2/3/2) → the next N open, unblocked unit ids. `… slots` /
  `… next N` / `… preflight` run the pieces alone.
- **`install-helpers/park-blocker.sh <ID> "<reason>"`** (DRAIN-5) — when a unit hits a
  live-infra/artifact/gate blocker you can't clear from a build, PARK it: flips the unit
  to `[!]`, surfaces it in `docs/NEEDS-OPERATOR.md`, exits 0 so the loop CONTINUES.
  Never stall a whole tick on one item.
- **`install-helpers/assert-own-worktree.sh`** (DRAIN-7) — every isolated subagent runs
  this as STEP-0; it REFUSES the shared `calm-ray-dcr8`/primary checkout so an agent can't
  edit/reset the coordinator's tree (it wiped an agent's work once, 2026-06-24). Each
  agent prompt MUST open with: *"STEP-0: run `./install-helpers/assert-own-worktree.sh`;
  abort if it exits non-zero."*

**Per tick:** `drain-coordinator.sh plan` → dispatch one disjoint unit per free slot,
each farm-only, each STEP-0'ing the worktree guard → on each completion
merge/cherry-pick + reclaim the worktree + relaunch (rearm, never batch-wait) →
`park-blocker.sh` anything blocked and move on → no-flinch (GUI/infra/gated picked on
the same footing as backend).

### What counts as "productive fill" when the disjoint-unit supply runs thin
The file-disjoint worklist-unit supply depletes wave by wave. When it is thin, KEEP
THE FARM BUSY with work that saturates cores without needing new disjoint units —
idle is the only wrong answer:
- **Full-workspace gate on the integration branch:** `cargo build --workspace
  --release` / `cargo test --workspace` / `cargo clippy --all-targets` of
  `datacenter-control` — validates the otherwise-ungated 160+-commit PR line; one
  full workspace build saturates a whole node by itself.
- **The RPM cut** (`cargo build --workspace --release && cargo generate-rpm`) — only
  PUBLISHING is operator-gated; the *build* is free farm work.
- **Per-crate test/clippy sweeps**, the nightly L1/L2/L3 suites
  (`automation/testbed/`), `/audit` passes.
- **The next worklist-drain wave** — re-run discovery for the next file-disjoint
  slice (crates not already in-flight).

### Detached / long builds (so they don't get killed)
Long farm builds get sandbox-killed as FOREGROUND bg-tasks (exit 143/144). Run them
`run_in_background: true` (or `nohup … &` detached) and monitor a log file. The farm
reconciler wipes untracked `vendor/birthright` mid-build → keep **vendor +
`generate-rpm` a tight sequence** (no long build between them).

## Phase 0 — Rescue pass (always first after the governance refresh)

Before new work, catch the project's recurring failure mode (shipped-but-dead /
mockup-only code). This is the single highest-value step.

1. **Dead-module grep** (`crates/**/src`): for each `pub mod`/`mod`, confirm an
   external `<mod>::` reference exists. A module with helpers + tests but no caller
   is **not done** — it's unreachable. List offenders.
2. **Stub/mock grep:** `rg 'todo!\(|unimplemented!\(|panic!\("not |coming soon|placeholder|demo_data'`
   across `crates/**/src`. Each hit is either real work or a mislabelled task.
3. **Reachability:** every feature must be reachable from a real entrypoint and
   *do something* when launched. There is **no `mde <subcommand>` dispatcher** —
   the surfaces are separate binaries (`magic-fleet`, `mde-files`, `mde-workbench`,
   `mde-voice-hud`, `mde-music`, `mde-musicd`, `mde-bus`); daemon logic lands as a
   `mackesd` worker / `mde-bus` subscription; iced GUIs via `update`/`view`/
   `subscription`. Smoke a binary with `timeout 3 cargo run -p <crate>` (or
   `timeout 3 ./target/debug/<bin>` once built).
4. **Boundary check:** run `./install-helpers/lint-mesh-boundary.sh` — no mesh-side
   crate may depend on a deleted desktop-shell crate (§6). A reintroduced shell dep
   is a rescue.
5. **Re-cue misleading `[✓]`:** any worklist item marked done but failing 1–4 flips
   back to `[>]` with a one-line note. If ≥3 rescues, write a short audit note.

## Phase 1–N — Drain loop

For each open `[ ]` task, highest priority first:

1. Mark `[>]` in the worklist (restart-safe claim).
2. Implement **fully** per §7 (Definition of Done) — no stubs, runtime-reachable,
   no raw hex / scattered metric literals outside the `mde-theme` token modules
   (`crates/shared/mde-theme`, §4). New code is **glue, not reimplementation** (§6):
   reuse the existing crates rather than re-deriving them.
3. **Gate before commit** (auto-fix in scope; SOFT-ESCAPE if the same fix fails 3×).
   Run from the repo root:
   - `cargo check --workspace` · `cargo build --workspace`
   - `cargo test` (and `cargo test -p mde-theme` for any Carbon token / palette /
     metric change)
   - `cargo clippy --all-targets` · `cargo fmt --all`
   - `./install-helpers/lint-mesh-boundary.sh` (the mesh/desktop boundary gate)
   - **Visual tasks (iced/Cosmic GUIs):** the operator/on-session visual-confirmation
     gate is **lifted (2026-06-11, operator directive)** — see §7. A GUI change is done
     when it builds, tests green, and renders through the `mde-theme` Carbon tokens (§4,
     still enforced: no raw hex / scattered metrics). `/preview` is optional/best-effort,
     never a blocker; do **not** hold a feature `[>]` solely for an on-Cosmic visual
     check.
   - Note: a full build needs the system dev libs (`sudo dnf install -y gtk3-devel
     alsa-lib-devel`) — the audio chain links ALSA. No crates are excluded; all 20
     workspace members build. `.cargo/config.toml` sets `CMAKE_POLICY_VERSION_MINIMUM=3.5`
     for the vendored Opus tree.
4. Commit named pathspecs with a why-not-what message + the `Co-Authored-By`
   trailer:
   `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
   Flip the task `[✓]`. **Do not push** — pushing is outward-facing and stays
   operator-gated.
5. Run independent tasks in parallel where they don't touch the same files.

**Commit cadence.** Work tasks in small local commits as above. Keep commits scoped
and individually runtime-reachable + stub-free (§7) before they land. If the operator
has locked a per-epic squash policy in `AI_GOVERNANCE.md` or a design doc, follow it;
otherwise default to small scoped commits and let the operator decide squashing at
push time.

## Stop conditions

Worklist empty (only gated items remain) · a push/release/cutover moment · a
destructive op · a product-direction change · two consecutive unexplained gate
failures · ≥10 rescues at once. On stop: a short factual summary + what's left.

Pushing is `git push origin master` only — single `origin` remote, branch `master`,
and only after explicit operator go-ahead. The RPM cut is always operator-gated
(`/release`).

## NOT this skill

Single obvious edit → just do it. Release cut → `/release`. Deep integrity sweep
with a written report → `/audit`. Visual render check → `/preview`. Design / survey
/ worklist authoring before code → `/plan`.
