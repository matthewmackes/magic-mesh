# Postmortem — the silent line divergence (why this work had to be reconciled)

**Date:** 2026-06-29
**Author:** the autonomous `/ship` drain (this session) — managed entirely by the
AI loop + the Xen build farm, which is exactly the point of this writeup.
**Severity:** would-have-been-high (a fleet downgrade) — caught before deploy.

---

## 1. What happened

When the operator authorized publishing/deploying the artifact this session had
built — `magic-mesh-11.0.8` — a pre-deploy version check found that the **live
fleet runs `11.0.14`** and the **active development trunk (`farm-autoscale-plan`)
is `11.1.0`**, while this session's line (now `master`) was `11.0.8`.

The two lines had **diverged at the common ancestor `c221f34`** and never
reconciled:

| | `master` (this line) | `farm-autoscale-plan` (fleet line) |
|---|---|---|
| version | 11.0.8 | 11.1.0 (fleet deployed: 11.0.14) |
| commits since `c221f34` not in the other | 79 | 210 |
| source files differing | — | **189** |
| files only on this line | 36 | — |
| files only on the other | — | 101 |

Deploying `11.0.8` to an `11.0.14` fleet would have been a **downgrade dropping
210 commits** — including the completed lighthouse migration, the `mackesd secret`
CLI, the overlay-IP collision fix, and the joined-lighthouse enrollment cert fix.
The deploy was halted and the operator chose to **reconcile the two lines** first.

The most telling symptom: **both lines independently implemented the same
worklist items** (DATACENTER plane, MEDIA, MOTION, the LH-JOIN-QNM guards). The
`ssh_pubkey_gossip` stray-write guard exists on *both* lines. Two teams of one —
the same author, in different worktrees — built the same features twice.

## 2. Why it happened — root cause

**Parallel autonomous worktree sessions drained one worklist off one base without
a shared integration trunk or routine rebasing.** Concretely:

- Multiple `/ship` sessions ran in separate git worktrees (`bright-elm-ajw0`,
  `calm-ray-dcr8`/`farm-autoscale-plan`, the `worktree-agent-*` and `farm-auto/*`
  branches), each **branched at or near `c221f34`**.
- Each session **treated its own branch as the world**: it drained
  `docs/WORKLIST.md`, implemented items, built + farm-validated, and merged to its
  own branch / PR — but **no step compared its branch to the other live lines or
  to the deployed fleet**.
- The same worklist items were therefore implemented **more than once, divergently**
  (each session re-derived DATACENTER/MEDIA/MOTION because its copy of the worklist
  showed them open). This is the [[worklist-drift-reconcile-first]] failure mode at
  whole-line scale.
- **Version numbers advanced independently** (`11.0.8` here; `11.0.9`→`11.0.15` on
  the other line) with no collision check, so nothing flagged that two lines were
  both claiming the `11.0.x` space.

## 3. Why "managed by me + the farm" did not catch it

This is the part worth internalizing: the failure was **invisible to both the AI
loop and the farm by construction.**

- **The `/ship` loop is per-worktree.** It optimizes "drain *this* branch to done."
  Its stop condition ("worklist empty, only gated items remain") was *true for each
  line in isolation* — so every session honestly reported "complete." Completeness
  per-branch is not completeness of the product when branches diverge.
- **The farm builds whatever branch it is handed.** It has no notion of a canonical
  line. It compiled, tested green, and cut RPMs for *both* divergent lines, which
  **reinforced the illusion** that each was a finished, deployable whole. A green
  farm build is a statement about one tree, not about agreement between trees.
- **The worklist itself diverged.** Each line edited its own `docs/WORKLIST.md`, so
  "the worklist says done" meant different things on different lines — the single
  source of truth had forked along with the code.
- **No deploy-time fleet-version gate existed.** Nothing compared the artifact's
  version to the fleet's running version, so the only thing standing between a
  downgrade and the fleet was a manual check that happened to be run this session.

The throughline: **autonomy + a build farm scale *output*, not *agreement*.** Two
independent agents producing green, complete-looking branches off a shared base
will silently fork unless something actively enforces convergence.

## 4. The solution

### Immediate (this session)
Reconcile to one trunk, fleet-version-safe:
1. Map the divergence per subsystem (a fan-out workflow) → a precise port/drop list.
2. **Base the unified trunk on the fleet-proven line** (`farm-autoscale-plan`,
   11.1.0) — never re-merge a parallel reimplementation over fleet-tested code.
3. Port only this line's *genuinely unique* work (e.g. the XEN-194 onboarding, the
   LH-JOIN-QNM mount regression test) onto that base.
4. Bump the trunk version *above* the fleet's, build, and deploy from it — so the
   roll is an upgrade, with the fleet's current NEVRA staged as the rollback RPM.

### Durable prevention (the real fix)
- **One integration trunk; rebase often.** Autonomous sessions must rebase onto /
  merge into a single trunk on a short cadence (≪ the divergence horizon), not
  branch-and-forget off a weeks-old base. Long-lived worktrees are the hazard.
- **A pre-"done" divergence gate.** Before a session declares done or builds a
  release, run the exact check that caught this: compare the branch (by `git cherry`
  patch-id) and its version against the canonical trunk **and the deployed fleet
  version**. Diverged > N commits or version ≤ fleet → stop and reconcile first.
- **A fleet-version downgrade guard in `/release` + the deploy path.** Refuse to
  publish/deploy any artifact whose version is ≤ the version currently running on
  the fleet. A downgrade must be an explicit, typed-confirm override.
- **Version-collision detection.** A version bump must check the highest existing
  tag and the fleet's running version; two lines must never both mint `11.0.x`.
- **Make the farm report agreement, not just green.** A farm gate that flags when
  the branch under build diverges from the canonical trunk / deployed version by
  more than a threshold — so "green" can never again be mistaken for "converged."
- **One worklist on the trunk.** `docs/WORKLIST.md` lives on the integration trunk
  only; worktrees read it, they do not fork their own copy of the source of truth.

## 5. The one-line lesson

> A build farm and an autonomous loop multiply **throughput**, not **convergence**.
> Without an active gate that compares every line to the canonical trunk and the
> deployed fleet, parallel "complete" branches fork silently — and the first time
> anyone notices is at deploy, as a downgrade. Enforce convergence explicitly.
