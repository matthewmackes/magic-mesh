---
name: plan
description: Design-thinking + survey + worklist-management skill for the MCNF mesh workspace. Use when the user asks to scope an epic, run an N-Q AskUserQuestion survey, audit the worklist for drift, rescue dead crate-modules, lift design-doc actions into worklist tasks, draft a design document, or otherwise PLAN work before any code lands. Sister skills: `ship` (drains the queue), `release` (the operator-gated RPM cut), plus `audit` + `preview`.
---

# Plan

The design + worklist-management skill. **Everything before code
lands runs through plan.** Surveys design forks via
`AskUserQuestion`, lifts design-doc actions into worklist tasks,
audits the worklist for drift, drafts new design documents.

## Triggers

- "Design [X]" / "Survey [X]" / "Lock [X]"
- "Audit the worklist" / "Rescue the worklist" / "Find dead modules"
- "Lift the design doc actions into the worklist"
- "Run an N-Q survey on [X]" / "Fire 25 questions about [X]"
- "Plan the next [epic]"

## Method

### Survey pattern (≥3-option design forks)

When the user asks to lock a non-trivial design decision (≥3
plausible options), fire an `AskUserQuestion` survey **one
question at a time**. Group into rounds (e.g., 10 questions per
round). After every round, recap the locks before proceeding.

After all questions:

1. Write `docs/design/<epic>.md` capturing every lock in a table
   + the resulting architecture + acceptance criteria + risks +
   out-of-scope items. (`docs/` does not exist yet — create it on
   first use.)
2. Lift every actionable item into `docs/WORKLIST.md` as a new
   `### EPIC-NAME` section with user-story tasks (As/I want/so that
   + runtime-observable acceptance bullets per the no-stubs rule —
   `AI_GOVERNANCE.md` §7). The worklist is the single durable
   tracker, created when execution begins; until then the plan doc
   + `AI_GOVERNANCE.md` are the trackers.
3. Update `AI_GOVERNANCE.md` (repo root) if the survey locks
   platform-wide direction (not just per-epic).
4. Commit + `git push origin master` only after explicit operator
   go-ahead (pushing is outward-facing). Single `origin` remote,
   branch `master` — there is no dual-remote push.

### Worklist rescue pass

Before any large ship effort, scan for:

- **Dead modules** — `pub mod foo;` in a crate under `crates/**`
  with zero external `foo::` / `crate::foo::` references (and no
  `pub use foo::*` re-export). Tests inside `foo.rs` itself don't
  count — they reference the module from within.
- **Misleading `[✓]` marks** — tasks marked done where the
  runtime-reachability gate (§7) doesn't actually hold: the code
  exists but no real entrypoint invokes it — no app-binary path
  (`magic-fleet`, `mde-files`, `mde-workbench`, `mde-voice-hud`,
  `mde-music`, `mde-musicd`, `mde-bus`), no iced
  `update`/`view`/`subscription`, no `mackesd` worker / `mde-bus`
  subscription.
- **Mockup-only features** — UI that renders but the underlying
  state never updates; `demo_data`/placeholder constants or
  "coming soon" strings standing in for real behavior.
- **Deferred markers** — code or worklist text saying "lands in
  a follow-up", "wired in Phase N", "deferred to", "stub for now",
  "todo!()", "unimplemented!()", `panic!("not yet …")`.
- **Boundary violations** — a mesh-side crate that picks up a
  dependency on a deleted desktop-shell crate (§6). Confirm with
  `./install-helpers/lint-mesh-boundary.sh`.
- **Design-doc actions never lifted** — items in
  `docs/design/*.md` that don't have matching worklist entries.

Each finding becomes a new worklist task (user-story shape) BEFORE
any new code lands. §7 (Definition of Done — no stubs,
runtime-reachable) is the upstream prevention; the rescue pass is
the downstream catch.

### Authority

When two locks contradict (newest wins silently):
1. **Memory** (`~/.claude/projects/-home-mm-magic-mesh/memory/*.md`)
   — operator live preferences, highest.
2. **`AI_GOVERNANCE.md`** (repo root) — the platform identity +
   architectural locks (the E11 "MCNF" pivot). This is the
   operational rulebook; there is no `CLAUDE.md` in this repo.
3. **`docs/design/*.md`** — the per-epic design locks.
4. **`docs/WORKLIST.md`** body — actionable state.

Newest wins. When in doubt: the §0 master rule from
`AI_GOVERNANCE.md` ("Secure, Simple, No-Fixed-Center Workgroup").

## Worklist schema

```
- [ ] **<PREFIX>-N.M: <epic> — <short title>** *(optional carve-out tag)*
  **As** <role>,
  **I want** <capability>,
  **so that** <outcome>.
  **Acceptance** (each runtime-observable):
    - [ ] specific runtime-observable bullet
    - [ ] specific runtime-observable bullet
```

Status legend: `[ ] Open`, `[>] In Progress` (carry a
`session=<id>` marker when a `/ship` session claims it),
`[✓] Done`, `[!] Blocked`. `[~] Deferred` is RETIRED — no silent
deferrals.

Every task carries an epic prefix tying it to the design docs. The
RPM is held until every feature is §7-complete and is always
operator-gated (`/release`).

## Companion skills

The live `.claude/skills/` set is exactly five — cross-reference
only these:

- `ship` — when planning is done, switch to ship to drain the
  queue (the autonomous loop + completeness rules).
- `release` — operator-gated RPM cut/push/tag flow; never
  auto-trigger it from a ship run.
- `audit` — integrity sweep of the mesh workspace (dead/unreachable
  code, stubs, boundary + Carbon-token violations) with
  FINISH-or-REMOVE verdicts.
- `preview` — visual / accuracy check for the iced Cosmic GUIs;
  verify a render actually looks right rather than trusting a green
  `cargo test`.
