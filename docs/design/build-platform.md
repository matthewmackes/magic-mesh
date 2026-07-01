# Build Platform — fast builds, least AI tokens, internal e2e/feature/stability testing

*Design locked 2026-06-22 (4-Q survey). Builds on the FARM-AUTO substrate + the
4-node IaC farm (XEN-HOME-SERVICES/.50, KVM-XCP1/.90, XEN-BIGBOY/.130, XEN-194/.170 —
9 heavy build slots; canonical roster `install-helpers/farm-topology.sh`) and the
bottleneck analysis that preceded it.*

## Goal

Turn the worklist → working signed RPM as fast and cheaply as possible: builds
happen because the worklist changed (not because an AI triggered them), the AI
spends tokens only on **design** and **failure triage**, and correctness is proven
by **internal** install / feature / stability testing on real VMs — no external CI,
no human in the build loop.

## Design locks

| # | Decision | Lock | Why |
|---|---|---|---|
| 1 | Canonical trigger lane | **GitOps reconciler on a timer** (`FARM-AUTO-4`) | Autonomous, zero-AI, worklist-driven — lowest token cost. The other 4 capabilities stay available (Forgejo for push-CI, mackesd/Bus for events) but the reconciler is the default the platform is built around. |
| 2 | Build-speed investment | **Shared `sccache`** (only) | Cold cargo-target was the dominant per-job latency; a shared cache lets any node reuse compiled artifacts. Per-node slots / warm-image / more-nodes are deferred (see Out-of-scope). |
| 3 | Test gate rigor + timing | **On-demand + nightly** | Heavy e2e/feature/stability tests run nightly + on manual trigger and **never block a build**. The fast path (build+unit) stays fast; safety net runs off the critical path. |
| 4 | E2E test environment | **Snapshot-reset VM pool from `MDE-VM-golden`** | OpenTofu spins clean VMs, installs the real RPM, runs acceptance, destroys them — real, isolated, reproducible; matches DEVOPS-SUBSTRATE. |
| 5 | AI role boundary *(defaulted from "least tokens")* | **Design + failure-triage only** | The fleet runs build/test/report autonomously; AI is invoked to design new work or diagnose a red result — never to dispatch or babysit a build. |
| 6 | Build cadence/artifact *(defaulted)* | Per-crate fast path on change + **nightly RPM cut** + operator `/release` | Fast feedback per change; the real install artifact (the signed RPM) is produced nightly and at release. |

## Architecture

```
worklist @farm jobs ──(reconciler timer, no AI)──▶ farm-dispatch ──▶ build on fleet (sccache-warm)
                                                                          │  results → Bus/panel
   nightly ──▶ RPM cut ──▶ snapshot-reset VM pool (tofu) ──▶ install + feature + stability acceptance ──▶ nightly report
```

### A. The canonical lane (build fast, no AI)
The `mcnf-farm-reconcile.timer` (FARM-AUTO-4) converges the worklist's active
`@farm` jobs onto the fleet every N min, idempotently (skips fresh results).
Per-crate `cargo build`/`test`/`clippy` jobs are the **L0** gate — fast feedback,
fully fleet-side. The AI never triggers a build; it tags a worklist task `@farm:{…}`
(design) and reads the result only if asked (triage).

### B. Build speed — shared sccache
- `sccache` on every build VM, pointed at a **shared backend** on the control host
  (an `sccache` server / a Mesh-Sync or NFS dir) so a crate compiled on any node
  is reused on every node. `RUSTC_WRAPPER=sccache` baked into the toolchain
  (Ansible) + the dispatch env.
- Kills the cold-target latency that dominated the production run. Measured target:
  a fresh-VM build of `mackesd`/`mde-workbench` from a warm cache ≪ the cold time.

### C. The internal testing pyramid (on-demand + nightly, on the VM pool)
| Tier | When | What | Where |
|---|---|---|---|
| **L0 build+unit** | every change (reconciler) | `cargo build`/`test`/`clippy`/`fmt` + mesh-boundary + Carbon-token lints, per crate | build VMs (sccache) |
| **L1 install (e2e)** | nightly + on-demand | cut the RPM → clean VM from `MDE-VM-golden` → **install the RPM** → assert: services up, role chooser, `found`/`join` enrol, overlay forms | snapshot-reset VM pool |
| **L2 feature** | nightly + on-demand | per-feature **runtime-observable** acceptance (the §7 bullets): a service exposes, mesh-DNS resolves, a GUI binary launches, etc. | VM pool / small ephemeral mesh |
| **L3 stability** | nightly/weekly | **soak** (daemon footprint flat under sustained traffic — the BUS-RETENTION pattern), **chaos** (destroy a lighthouse → no FUSE wedge, failover holds — the INCIDENT-WEDGE lesson), **reboot-recovery** (BOOT-REC: mounts/overlay self-heal) | VM pool |
All tiers write results to the **Bus** (`event/farm/*`, `event/test/*`) → the
Workbench Build panel (FARM-AUTO-5) + a nightly summary. L1–L3 never block L0.

### D. The token-budget model (least AI tokens)
- **No AI in the build/test loop** — the reconciler + VM pool run on timers; the AI
  is not a scheduler.
- **AI offloads, never compiles** — when the AI does need a build (design/verify),
  one `farm-dispatch` call runs it on the fleet; the AI pays a tool-call, not a
  compile.
- **Failures pull the AI in, success is silent** — a red nightly raises a Bus alert;
  the AI is invoked to triage only then.
- **Heavy work is nightly, not per-turn** — install/feature/stability never run in an
  AI turn.

## Bottleneck → disposition

| Bottleneck (found) | This plan |
|---|---|
| Cold cargo target | **Fixed** — shared sccache (lock 2) |
| Per-job rsync | Mitigated by sccache (less to rebuild); a shared source is a later option |
| Node-count ceiling / whole-node lock / coarse scheduling | **Deferred** (Out-of-scope) — per-node slots + more nodes are the next lever once sccache lands |
| RAM cap per node | Respected — slots (deferred) would be memory-sized |
| Control-host hub | Acceptable at this scale; the pull-agent lane (#3) is the escape hatch if it bites |
| Runner serialization (#2) | N/A — reconciler is canonical, not Forgejo |
| **Dependency-gating (the real ceiling)** | **Acknowledged, not a compute problem** — the platform parallelizes *verification*; gated tasks (iced-0.14, live-fleet, operator inputs) stay serial. The plan does not pretend to fix this. |

## Acceptance criteria (each runtime-observable)
- A worklist `@farm` tag → a build on the fleet within one reconciler interval, **no
  AI involved**; result on the Bus + panel.
- A second build of the same crate on a *different* VM reuses the sccache (cache-hit
  rate observable via `sccache --show-stats`).
- A nightly run: RPM cut → clean VM install → install+feature acceptance → report,
  with a red result raising a Bus alert.
- A stability run kills a lighthouse and asserts no fleet-wide FUSE wedge + failover.
- An AI session can go from "tag a task" to "read pass/fail" spending only a tag edit
  + one status read — no compile tokens.

## Risks
- **sccache cache poisoning / staleness** — pin the toolchain (already 1.94.0); scope
  the cache by rustc version + target.
- **VM-pool flakiness** — snapshot-reset must be hermetic (fresh VM per run) or L1/L2
  go flaky; reuse the proven golden-template + NM-keyfile path.
- **Nightly drift** — if nightly is red for days unnoticed, the safety net rots;
  the Bus alert + a visible panel badge are load-bearing.
- **sccache won't help the linker** — mold/gold link time is separate; sccache caches
  compilation, not linking.

## Out of scope (now — explicit, not forgotten)
- Per-node job slots, warm-target golden image, more nodes/hardware (the next
  build-speed levers after sccache proves out).
- Forgejo-per-job ephemeral runners; mackesd-native event triggering as the default.
- Fixing dependency-gating (a planning/sequencing problem, not a build-platform one).
