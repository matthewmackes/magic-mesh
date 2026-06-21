# Process Governance — repeatable design → test → build → remediate

> **Status:** LOCKED (survey Q1–Q57, 2026-06-21). Landed as **AI_GOVERNANCE.md §10**
> (per Q1); two supersede entries in `docs/DECISIONS.md`; the infra epic + process
> tooling lifted into `docs/WORKLIST.md` under `### PROCESS` and `### DEVOPS-SUBSTRATE`.
> Goal: a tight, visible, enforceable process so effort stops going to problems
> that didn't need solving.

## Locks (running)

| # | Topic | Lock |
|---|-------|------|
| Q1 | Process home | New **§10 in AI_GOVERNANCE.md** (single high-authority home). |
| Q2 | Process form | Written as **numbered gates with binary pass/fail** (same idiom as build gates). |
| Q3 | Entry filter | Work starts only when **tracked (worklist task) AND symptom-backed** (concrete observed trigger). No speculative or untracked work. |
| Q4 | Design trigger | Full design doc required on a **structural trigger**: new crate, cross-crate (≥2 crates), or new surface. Localized changes go straight to a worklist task. |
| Q5 | Design doc spec | Mandatory sections = current 5 (locks table, architecture, acceptance, risks, out-of-scope) **+ explicit Non-Goals/Won't-Do**. |
| Q6 | Design→worklist link | **Bidirectional, lint-enforced**: doc rows carry worklist task IDs; tasks carry doc paths; a lint verifies the mapping is total. |
| Q7 | Lock immutability | A lock reopens only on **a concrete new symptom + a superseding lock with dated rationale**. Otherwise it stands; agents cite and move on. |
| Q8 | Gate single-source | One executable **`verify-gates.sh` IS the gate**; CI, CONTRIBUTING, and §10 all invoke/cite it. |
| Q9 | Gate cadence | **Every commit must be green** against the full gate (every point in history buildable + §7-reachable). |
| Q10 | Reachability proof | §7 done requires **naming the concrete entrypoint AND a test that exercises that path** — not just a claim. |
| Q11 | Remediation loop | **Reproduce → failing test → fix → gate**. The regression test stays as a permanent guard. |
| Q12 | Finish vs remove | Dead/mock/incomplete code defaults to **REMOVE** unless a worklist task commits to finishing it (with a date). Bias to a smaller, honest tree. |
| Q13 | Detour rule | **File Y, finish X, never detour.** Unrelated defects become their own symptom-backed tasks; a blocking Y marks X `[!] Blocked`. |
| Q14 | Visibility | A **generated status dashboard** renders worklist state at a glance; the `[ ]/[>]/[✓]/[!]` legend is the underlying signal. |
| Q15 | Worklist hygiene | **Archive done epics** to `docs/WORKLIST-archive.md`; live worklist holds only open/in-progress/blocked + recently-done. |
| Q16 | Stop rule | **Stop when the approach has been substantially rethought twice** without passing — escalate on thrash. |
| Q17 | Escalation packet | Escalation = **the symptom + an `AskUserQuestion` multiple-choice decision**. One read, one answer. |
| Q18 | §10 structure | **Four phase-blocks** (Design / Build+Test / Verify / Remediate), gates numbered within. The pillars are the headings. |
| Q19 | Gate labels | **Phase-letter + number (D1, B2, V3, R4)** — stable, citable, sortable. |
| Q20 | Commit contract | Each commit carries **task ID + the entrypoint made reachable + why-not-what**. Git history is the audit trail. |
| Q21 | Authority on forks | **Agent is the source of truth and locks the requirements** — for a ≥3-option fork the agent decides, documents rationale, and locks; operator is not gating each fork. |
| Q22 | Integration model | **Direct commits to master** (each green per Q9). |
| Q23 | Gate enforcement | **Blocking pre-commit hook running the full gate** — no red commit can exist; accepts the per-commit time cost. |
| Q24 | EFF-18 | **Fix EFF-18 at the root** (inject config, drop process-global env), restore parallel mackesd tests so the per-commit gate stays fast. (→ worklist prerequisite.) |
| Q25 | Lock firmness | **Same reopen rule for all locks** (§0–§9 and process gates): concrete new symptom + dated superseding lock. |
| Q26 | Docs timing | **Docs (ADMIN/help/CHANGELOG) update in the same commit** as the behavior change — never lag code. |
| Q27 | Transition code | **Dual-path allowed only with a tracked cutover task + removal date.** No open-ended dual maintenance. |
| Q28 | Test depth | **Integration test across the real cross-crate path** mandatory for Q4-trigger features. |
| Q29 | Incident feedback | Each incident yields **a regression guard AND a one-line governance lock** capturing the invariant. |
| Q30 | Preventive work | Preventive/hardening work allowed **only when it maps to an existing §8/§3 governance security control** — otherwise file and wait. |

### Self-locked mechanical gates (agent as source of truth, Q21)
- `verify-gates.sh` is the single gate definition; CI runs it verbatim (Q8).
- Dashboard = a script over the live worklist legend, run on demand (Q14).
- Archive file `docs/WORKLIST-archive.md` (Q15); new lint `lint-design-worklist-link.sh` (Q6).
- Remediation/bugfix needs no design doc unless it trips the Q4 structural trigger.

| Q31 | Machine authority | **Full autonomous control of every machine** — no production exists; the whole environment is **airgapped dev**. No live-ops gating. |
| Q32 | Verify tiers | Three tiers: **local cargo** (logic) · **ephemeral container mesh** (multi-node paths) · **real-VM mesh on the two XCP-ng hosts** (full fidelity). Build out a VM fleet + a DevOps management layer. |
| Q33 | DevOps layer | **Declarative IaC: OpenTofu/Terraform (Xen Orchestra provider) + Ansible** — git-versioned, repeatable VM spin-up/teardown. |
| Q34 | CI location | **Fully self-hosted CI** on the XCP-ng/agent fleet; GitHub is only the git remote. Cloud CI dropped. |
| Q35 | CI engine | **Forgejo/Gitea Actions** — Actions-compatible YAML; the existing `ci.yml` ports with minimal change. |
| Q36 | Git remote | **GitHub stays canonical** (release pipeline intact); **Forgejo pull-mirrors it** for CI. |
| Q37 | Test topology | Standard multi-node gate = **3 LH + 3 peers**; full **3 LH + 9 peer** envelope is a release gate. |
| Q38 | WIP limit | **Unbounded** — agent fans out as it sees fit; gates + no-detour + worktree isolation contain sprawl. |
| Q39 | Parallel isolation | **Each agent in its own worktree; merges to master serialized** (one fast-forward at a time, re-gate on conflict). |
| Q40 | Container vs VM | The multi-node gate runs **always on real XCP-ng VMs** (fidelity); fast VM provisioning via the IaC layer keeps it from bottlenecking. |

| Q41 | Rollout | **Infra is the first epic governed by §10** — the process dogfoods itself, bootstrapping its own substrate under the no-infra gates. |
| Q42 | Secrets | **Mesh-native: etcd + age over Nebula** (D-W1). Bootstrapping caveat tracked in the infra epic. |
| Q43 | Rollback | **Fix-forward only** — never revert; roll a new fix through the Q11 loop (which also strengthens the gate). |
| Q44 | Operator alerts | **Push only escalations + release-ready**; all other state is pull (dashboard). |
| Q45 | Non-Rust gate | **Lint shell now** (shellcheck/shfmt); IaC linters (ansible-lint, `tofu validate/fmt`, yamllint) phase in with the infra epic. |
| Q46 | Retroactivity | **Forward-only, backfill on touch** — legacy docs/tasks come up to spec when next edited. |
| Q47 | Governance edits | **Agents may edit `AI_GOVERNANCE.md`**; every § change needs a symptom + dated entry in `docs/DECISIONS.md` (append-only ADR log). |
| Q48 | VM lifecycle | **Persistent pool, reset-to-snapshot** between runs; periodic golden rebuild bounds drift; release gate does a full golden rebuild (clean-install fidelity). |
| Q49 | Test-first | **Test-first for features too** — failing acceptance/integration test written from the design criteria before implementation. |
| Q50 | Release cadence | **Milestone-based** — an epic reaching full §7-completion is a release candidate; operator still triggers `/release`. |

| Q51 | Fast lane | **Trivial fixes** (typos, dead imports, fmt/clippy nits) skip the task/symptom requirement but must pass the gate. |
| Q52 | Visual regression | **Pixel-diff visual gate added** — supersedes the §7 visual-lift directive (symptom: deterministic headless capture now feasible). Determinism via pinned fonts + software rasterizer + fixed resolution. |
| Q53 | Golden bless | **Re-bless goldens in the same commit**; image diff visible in git. Intentional UI changes self-document. |
| Q54 | Priority model | **Dependency-topological, then oldest.** Tasks carry `depends-on: <task-id>`. |
| Q55 | Coverage | **Diff-coverage floor on changed code** (90% via `cargo-llvm-cov` diff), not whole-repo. |
| Q56 | CLI parity | **Hard gate** — a new surface needs its CLI-parity path + a CLI test. |
| Q57 | Decomposition | **Epic = a releasable capability; task = smallest unit that independently passes the full gate.** |

_(survey closed at Q57 — genuine no-clear-answer forks exhausted; remaining details self-locked as source of truth per Q21)_

## Architecture (resulting process)

```
                    ┌─────────────────────────────────────────────┐
   symptom + task → │ ENTRY  E1 tracked · E2 symptom-backed        │  (trivial fast-lane)
                    └──────────────────────┬──────────────────────┘
                                           ▼
   structural trigger? ───yes──► DESIGN  D1 doc(+Non-Goals) · D2 agent locks forks
        (new crate /              D3 doc↔worklist lint · D4 epic/task split
         ≥2 crates / surface)                │
                                           ▼
                    BUILD+TEST (every commit, blocking pre-commit hook = verify-gates.sh)
                    B1 build · B2 clippy · B3 fmt+shell · B4 tests (test-first)
                    B5 lint gates · B6 deny · B7 diff-cov≥90 · B8 visual · B9 docs · B10 msg
                                           │
                                           ▼
                    VERIFY (before [✓])   V1 entrypoint+test · V2 integration
                    V3 CLI parity · V4 real-VM 3LH+3p · V5 no stubs
                                           │
                                  ┌────────┴────────┐
                              pass [✓]          defect found
                                  │                 ▼
                                  │      REMEDIATE  R1 repro→test→fix→gate
                                  │      R2 remove-default · R3 no detour · R4 cutover-date
                                  │      R5 fix-forward · R6 incident→guard+lock · R7 escalate@2
                                  ▼
              milestone: epic §7-complete ──► release candidate ──► /release (operator-gated)
```

- **One executable gate.** `install-helpers/verify-gates.sh` is the single definition of
  B1–B8; the pre-commit hook and CI (self-hosted Forgejo Actions) both invoke it. No
  prose copy of the gate list is authoritative.
- **Single high-authority home.** The law is `AI_GOVERNANCE.md §10`; this doc is the
  rationale + full Q-lock map; `docs/DECISIONS.md` is the append-only reopen log.
- **Infra substrate** (first epic): OpenTofu (Xen Orchestra) + Ansible stand up the
  VM fleet on the two XCP-ng hosts; a snapshot-reset pool serves the V4 real-VM gate;
  Forgejo (pull-mirror of GitHub) runs CI; secrets ride etcd + age over Nebula.

## Acceptance (runtime-observable)

- [ ] `install-helpers/verify-gates.sh` exists, exits non-zero on any failing gate, and
      is invoked verbatim by both the pre-commit hook and CI (grep both call sites).
- [ ] A commit that omits its task ID / entrypoint, leaves a stub, or drops diff-coverage
      below 90% is **rejected by the pre-commit hook** (demonstrated on a deliberate red).
- [ ] `lint-design-worklist-link.sh` fails when a design-doc action row has no task ID or
      a task names a non-existent doc.
- [ ] The dashboard script renders `[ ]/[>]/[✓]/[!]` counts from the live worklist.
- [ ] A mesh feature cannot reach `[✓]` without a passing 3 LH + 3 peer real-VM run.
- [ ] An intentional UI change updates its golden image in the same commit; an
      unintentional pixel diff fails B8.
- [ ] `docs/DECISIONS.md` gains an entry whenever an `AI_GOVERNANCE.md` § lock changes.

## Risks

- **Bootstrapping circularity** — secrets (etcd+age) + the V4 gate live on infra the first
  epic is still building. Mitigation: infra-dependent gates are explicitly *phased in*
  (rollout lock); they don't block until their substrate exists.
- **Pre-commit hook latency** — the full gate per commit (esp. mackesd tests + visual)
  could be slow. Mitigation: EFF-18 root-fix restores test parallelism; visual + real-VM
  gates run deterministically and are scoped to relevant changes.
- **Self-hosted CI single-environment** — dropping cloud CI concentrates risk on the
  fleet. Mitigation: GitHub stays canonical so the repo + release pipeline survive a
  fleet outage; CI is reproducible from IaC.
- **Pixel-diff flakiness** — the classic reason the visual gate was lifted. Mitigation:
  pinned fonts + software rasterizer + fixed resolution; structural fallback if flaky.

## Out-of-scope

- Multi-tenant / hyperscale CI, multi-CA HA build infra (stays within the §8 envelope).
- Replacing the existing architectural locks §0–§9 (this epic adds §10; it does not
  reopen substrate/crypto/Carbon/boundary).
- A GUI for the dashboard (a text/terminal render is sufficient).

## Non-Goals (pre-committed: we will NOT build)

- **No bespoke CI engine / orchestrator** — Forgejo Actions + OpenTofu + Ansible only; no
  Kubernetes control plane, no custom scheduler.
- **No per-commit full-envelope (3+9) mesh** — that fidelity is a release gate, not a
  per-feature one.
- **No manual visual sign-off as a blocker** — visual correctness is the automated B8
  gate or nothing; `/preview` stays optional.
- **No retroactive backfill epic** — legacy docs/tasks converge on-touch, not in a sweep.
- **No revert-based rollback tooling** — fix-forward only (R5); we will not build a
  revert/feature-flag rollback system.
- **No speculative hardening** — preventive work exists only against a §3/§8 control.
