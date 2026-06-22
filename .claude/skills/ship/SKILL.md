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
subagents across `.50/.51/.52`, never grind solo or serialize decomposable work)**,
§4 (Carbon look), §6 (the mesh boundary), and §7 (Definition of Done). If you catch
yourself building locally/sequentially when the work could go to the farm in
parallel, that's a §10.0 violation — fix it before continuing.

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
