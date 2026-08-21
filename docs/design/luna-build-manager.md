# Luna — dependable 24-hour build manager and worklist workhorse

**Status:** Design for implementation
**Owner:** Build platform / farm automation
**Authority:** `AI_GOVERNANCE.md` §7/§10, `docs/BUILD-ENVIRONMENT.md`, `docs/design/build-platform.md`, `docs/design/devops-automation-rebuild.md`
**Purpose:** turn the existing farm from a set of scripts into a dependable, always-on build-management system that drains the worklist continuously, proves results with evidence, recovers from partial failure, and never depends on an AI babysitter.

Luna is **not** a new central scheduler. It is the management layer that makes the existing governed pieces dependable:

- `install-helpers/farm-topology.sh` remains the canonical fleet roster.
- `automation/lib/farm-dispatch.sh` remains the slot-aware executor.
- `automation/reconciler/farm-reconcile.sh` remains the canonical `@farm` convergence lane.
- Forgejo Actions remains the self-hosted CI control plane.
- etcd remains the durable coordination store.
- `mackesd` remains the platform authority and audit surface.

Luna adds the missing dependable-management contract around them.

---

## 1. Goals

1. **Continuous drain:** every runnable worklist unit is dispatched, verified, and reconciled without manual kicks.
2. **Dependability:** a crash, stale agent, wedged node, full disk, or duplicate dispatch is detected and corrected automatically.
3. **No duplicate work:** identical job IDs and identical commands at the same commit are never run twice.
4. **Capacity-aware execution:** jobs are admitted only when the target slot can actually hold the work.
5. **Evidence-first completion:** no worklist item is considered done without a result record, log, and evidence pointer.
6. **24-hour operation:** Luna must run unattended through retries, node churn, and agent failures.
7. **No AI in the steady-state loop:** AI is used for design, triage, and repair, not for routine scheduling.

---

## 2. Non-goals

- No Jenkins/Buildkite/GitLab/Nomad/Kubernetes control plane.
- No second scheduler competing with the reconciler.
- No raw shell execution on seats as a normal work path.
- No “AI watches the farm” model.
- No hidden mutable state outside the governed repo/state directories.

---

## 3. Current failure modes Luna must eliminate

Observed in production on 2026-08-21:

1. **Stale implementation worktrees were treated as active work** because `native-agent-dispatch.sh` used directory existence as its only liveness test.
2. **Duplicate dispatches** occurred when multiple reconciler trees saw the same job.
3. **A wedged node could hold a reservation** because SSH probes were not bounded end-to-end.
4. **Disk, not CPU, was the real constraint**; nodes that looked “ready” could not admit heavy workspace jobs.
5. **The monitor/noise path matched historical log lines** and produced false fanout alerts.
6. **Manual salvage was required** because the system had no durable lifecycle state for implementation agents.

Luna exists to make those states impossible to misread and unnecessary to fix by hand.

---

## 4. Architecture

Luna is a **management plane** with four cooperating components:

```text
WORKLIST.md
   │
   ▼
agent-dispatch.sh ──▶ native-agent-dispatch.sh ──▶ isolated implementation worktrees
   │                         │
   │                         ├─ lifecycle state: queued/running/blocked/completed/stale
   │                         ├─ heartbeat: pid, start time, log mtime, last result
   │                         └─ salvage/requeue policy
   │
   ▼
farm-reconcile.sh ──▶ farm-dispatch.sh ──▶ farm slots on build VMs
   │                         │
   │                         ├─ slot reservation
   │                         ├─ disk-shaped admission
   │                         ├─ bounded probes
   │                         └─ result JSON + logs
   │
   ▼
Forgejo Actions (farm runner) ──▶ CI gates / status publication
   │
   ▼
etcd / Bus / evidence files ──▶ durable state, audit, and reporting
```

### 4.1 Component roles

| Component | Role | Must not do |
|---|---|---|
| `farm-topology.sh` | canonical node roster and capacity | drift from live farm |
| `farm-dispatch.sh` | execute one job on one slot | schedule policy beyond admission |
| `farm-reconcile.sh` | converge active `@farm` jobs | implement agent lifecycle |
| `agent-dispatch.sh` | plan and invoke implementation adapter | invent prompts or bypass ownership |
| `native-agent-dispatch.sh` | create isolated worktrees and launch native agents | treat directory existence as liveness |
| Forgejo Actions | CI orchestration and status publication | become the only source of truth for worklist state |
| Luna manager | lifecycle accounting, recovery, evidence, and reporting | replace the dispatcher or reconciler |

---

## 5. Luna state model

Luna introduces a durable, explicit lifecycle for every work item.

### 5.1 Work item states

| State | Meaning | Entry condition |
|---|---|---|
| `queued` | eligible for dispatch | active worklist unit with `@farm` marker |
| `dispatching` | reservation or agent start in progress | dispatcher invoked |
| `running` | work is actively executing | live PID or active slot reservation |
| `blocked` | cannot proceed without a named blocker | explicit blocker evidence |
| `completed` | result recorded and accepted | pass/fail result + evidence written |
| `stale` | worktree or process appears abandoned | heartbeat timeout or dead PID |
| `salvaged` | work preserved for review/retry | diff/log archived |
| `requeued` | eligible for another attempt | stale/blocked item reset with evidence |

### 5.2 Required per-item fields

Each item record must carry:

- `job_id`
- `epic`
- `command`
- `runtime` (`cursor`, `codex`, `claude`)
- `worktree`
- `pid` (when running)
- `started_at`
- `heartbeat_at`
- `last_log_mtime`
- `result_path`
- `evidence_path`
- `commit`
- `status`
- `failure_class` (`infra`, `source`, `capacity`, `agent`, `unknown`)
- `retry_count`

### 5.3 Liveness rule

A worktree is **not** evidence of life.

A unit is `running` only if:

1. its recorded PID is alive, or
2. its farm slot reservation is held and the remote command is active, or
3. its heartbeat/log mtime is fresh within the configured window.

Anything else is `stale` and eligible for salvage/requeue.

---

## 6. Scheduling and admission

Luna keeps the existing slot-aware dispatcher as the execution engine.

### 6.1 Slot model

- One slot = one lock + one isolated remote workspace.
- Slot capacity comes from `farm-topology.sh`.
- BigBoy remains the long-pole node.
- `.170` and `.196` remain light-only until their storage grows.

### 6.2 Admission policy

Admission must check, in order:

1. node reachability
2. toolchain readiness
3. free disk headroom
4. slot reservation availability
5. shape fit (`heavy` vs `light`)
6. duplicate suppression by job ID and command

### 6.3 Disk-shaped admission

- `heavy`: whole-workspace, release, rpm
- `light`: per-crate build/test/clippy
- heavy jobs require large headroom
- light jobs require smaller headroom
- every additional reserved slot on a node raises the required envelope

### 6.4 Bounded probes

All remote probes must be time-bounded end-to-end, not merely connection-bounded.

A node that accepts TCP but cannot complete SSH banner/login is **not ready**.

---

## 7. Duplicate prevention

Luna must enforce two layers of duplicate prevention:

1. **Per-job-id idempotence** — the same worklist unit cannot run twice concurrently.
2. **Per-command dedupe** — identical commands at the same clean commit reuse the existing result.

A duplicate request must wait for the owner and adopt the result, not burn another slot.

---

## 8. Agent lifecycle management

This is the core missing piece Luna adds.

### 8.1 Launch contract

For every implementation agent, Luna records:

- runtime
- job id
- epic
- command
- worktree path
- PID
- start time
- prompt hash
- source commit

### 8.2 Heartbeats

A running agent must update at least one of:

- process liveness
- log mtime
- explicit heartbeat file

If none are fresh, the item becomes `stale`.

### 8.3 Completion

An agent is complete only when:

- it exits successfully, and
- its result is recorded, and
- its evidence file exists, and
- the parent coordinator can review or fold the diff

### 8.4 Stale recovery

When an item goes stale:

1. preserve the diff and log outside the repo
2. remove the abandoned worktree registration
3. mark the item `salvaged`
4. requeue if still runnable
5. record the reason and timestamp

### 8.5 Blocked recovery

A blocked item must name the blocker class:

- `capacity`
- `source`
- `infra`
- `agent`
- `operator`

Blocked items are not retried blindly; they are surfaced for correction.

---

## 9. Checks and management

Luna must expose checks that are cheap, deterministic, and machine-readable.

### 9.1 Health checks

| Check | Question answered |
|---|---|
| `luna status` | what is queued/running/blocked/completed? |
| `luna doctor` | what is misconfigured or stale? |
| `luna slots` | what capacity exists right now? |
| `luna evidence <job>` | where is the proof? |
| `luna salvage <job>` | preserve and requeue a stale unit |
| `luna reap` | clean abandoned worktrees and dead reservations |

### 9.2 Invariants

Luna must continuously assert:

- no duplicate active job IDs
- no duplicate active commands at the same commit
- no stale worktree is treated as active
- no node is admitted without disk headroom
- no result is accepted without evidence
- no farm script is edited in place while running
- no agent is considered complete without a result record

### 9.3 Logging and observability

Every state transition must emit a structured log line:

- `ts`
- `job_id`
- `epic`
- `from_state`
- `to_state`
- `reason`
- `node` / `slot` when applicable
- `evidence_path` when applicable

---

## 10. Recovery behavior

Luna must recover forward, not roll back blindly.

### 10.1 Node wedge

If a node wedges:

1. stop admitting new work to it
2. preserve evidence
3. reboot or repair from dom0 if authorized
4. reclaim disk
5. rejoin only after readiness checks pass

### 10.2 Agent crash

If an agent crashes:

1. mark stale
2. salvage diff/log
3. requeue if the worklist unit remains open
4. preserve the failure class for triage

### 10.3 Disk pressure

If a node approaches the admission floor:

1. reclaim only provably idle rebuildable trees
2. never delete a reserved slot’s workspace
3. never delete a source tree
4. re-probe once, then skip if still short

### 10.4 Duplicate supervisor trees

If two supervisors race:

1. the job-id lock wins
2. the loser waits and adopts the result
3. no second slot is consumed

---

## 11. Evidence model

Every completed item must leave evidence that survives the turn.

### 11.1 Evidence requirements

A completed item must have:

- result JSON
- build/test log
- evidence markdown or equivalent record
- source commit
- node/slot/workspace metadata
- outcome
- duration

### 11.2 Evidence storage

Evidence belongs in the governed repo or governed state store, not in ad-hoc scratch only.

Recommended paths:

- `automation/.state/results/<jobid>.json`
- `automation/.state/logs/<jobid>.log`
- `docs/platform/evidence/<epic>-<date>-<slug>.md`

---

## 12. 24-hour workhorse behavior

Luna must be safe to run continuously.

### 12.1 Steady-state loop

Every tick:

1. read worklist
2. classify runnable units
3. reconcile stale/blocked/running/completed states
4. dispatch only missing work
5. verify capacity
6. publish status

### 12.2 Backoff policy

- saturated farm: back off
- repeated infra failure: increase delay and surface triage
- source failure: do not retry without a code change or explicit requeue

### 12.3 Quiet success

A healthy farm should be boring:

- no repeated historical-log alerts
- no duplicate dispatch noise
- no false “already dispatched” spam
- no hidden stale worktrees

---

## 13. Security and governance

Luna must preserve the existing locks:

- no new central scheduler
- no new MDE-private D-Bus names
- no raw shell on seats
- no unbounded remote execution
- no unreviewed auto-commit of unfinished agent work
- no secret material in repo or logs

All destructive farm actions must be explicit, recorded, and limited to authorized infrastructure.

---

## 14. Implementation plan

### Phase 1 — Lifecycle truth

- add durable per-item state store
- add heartbeat/liveness detection
- add stale/salvage/requeue transitions
- add structured status output

### Phase 2 — Dependable dispatch

- keep slot-aware dispatcher
- keep command/job dedupe
- keep disk-shaped admission
- add bounded probes everywhere
- add timeout handling for sync and build phases

### Phase 3 — Evidence and reporting

- require evidence for completion
- publish status summaries
- add `luna doctor` and `luna status`
- add invariant checks

### Phase 4 — Continuous operation

- run under the existing supervisor/timer model
- add safe requeue and retry policy
- add 24-hour soak validation

---

## 15. Acceptance criteria

Luna is ready only when all of the following are true:

1. a stale worktree is never mistaken for a live agent
2. a duplicate job ID or command cannot consume a second slot
3. a wedged node cannot hold a reservation indefinitely
4. a heavy job is admitted only when disk headroom is sufficient
5. every completed item has result + log + evidence
6. the system can run unattended for 24 hours without manual salvage
7. the farm can drain the worklist without false fanout alerts
8. the design is implementable against the current repo without a new central scheduler

---

## 16. Initial verification checklist

- `farm-dispatch.sh --self-test`
- `farm-reconcile.sh --self-test`
- `farm-jobs.sh --self-test`
- `agent-dispatch.sh --self-test`
- `ship-coordinator.sh --self-test`
- `lint-doc-supersession.sh`
- `lint-worklist.sh --self-test`
- live farm smoke: one light job and one heavy job on separate slots

---

## 17. Design decision

The build manager should be **dependable because it is explicit about state, evidence, and recovery**, not because it adds another platform.

Luna is the management contract that makes the existing farm behave like a 24-hour workhorse.
