# FARM-AUTOSCALE — demand-driven elastic build farm (tofu lifecycle)

**Operator directive (2026-06-24):** *"Using Tofu, add auto-scaling and full
lifecycle to the VM and POD resources in the FARM. A very large heavy-resource VM
should be available for builds on BigBoy, but spun down when several VMs need the
same hardware for normal build VMs or agents — and likewise on the two smaller Xen
hosts. The pipeline should create and destroy resources for the best on-demand use
of the hardware."*

Today the farm is **4 always-on, fixed-size build VMs** (`mcnf-build-50/51/52/53`,
build-VM IPs `.50`/`.90`/`.130`/`.170`, one per dom0 — canonical roster
`install-helpers/farm-topology.sh`). The first three are declared in `infra/tofu/`; the
4th dom0 **XEN-194** (`.170`) is live but **not yet in the elastic tofu model**
(`infra/tofu/variables.tf` validates only 3 dom0 keys — a known IaC gap). The directive
makes each dom0 **elastic**: it runs *either* one hardware-maxing big VM *or* several
small VMs / agent pods, created and destroyed by tofu to fit the live build workload.

## Locks (operator survey, 2026-06-24)

| # | Decision | Lock |
|---|---|---|
| L1 | Scaling trigger | **Demand from the build queue** — an autoscaler reads the worklist's `@farm` job queue; a heavy/whole-workspace build → a big VM; many small per-crate builds or agent jobs → several small VMs. No tags to maintain. |
| L2 | Orchestrator | **Extend the FARM-AUTO reconciler** — the no-AI GitOps reconciler (DRAIN-ENGINE / §10) gains a `farm-autoscaler` reconcile step that runs `tofu apply`/`destroy` from the live queue. No AI tokens in the loop. |
| L3 | Lifecycle | **Warm pool, scale-to-zero + snapshot-reset** — build VMs are clones of `MDE-VM-golden`, scaled up on demand, snapshot-reset to a clean post-toolchain baseline between jobs, scaled to **zero** when idle. tofu owns the count/shape. |
| L4 | Contention | **Mutually-exclusive shapes, demand wins** — a dom0 runs EITHER 1 big VM OR N small VMs; the autoscaler destroys one shape to free the hardware for the other, decided by live queue priority. |

## The dom0 substrate (cold facts)
| dom0 | host | capacity | big shape (≈whole host) | small shape |
|---|---|---|---|---|
| **XEN-BIGBOY** | 172.20.145.165 | 12c / 32 GiB · 398 GiB SR | `mcnf-build-big-52` ~10 vCPU / 26 GiB | N× 4 vCPU / 8–16 GiB |
| XEN-HOME-SERVICES | 172.20.0.9 | 4c / 24 GiB | ~3 vCPU / 18 GiB | 1–2× small |
| KVM-XCP1 | 172.20.145.193 | 4c / 23 GiB | ~3 vCPU / 18 GiB | 1–2× small |
| XEN-194 *(4th dom0)* | 172.20.145.194 | 4c (RAM/SR not cold-verified) | ~3 vCPU | 1–2× small |

**XEN-194** is the farm's 4th dom0 (build VM `.170`, heavy cap 2), verified live
2026-07-01 — a 4-core host like XEN-HOME-SERVICES / KVM-XCP1, but its RAM/SR are not yet
cold-verified and it is **not yet in the elastic tofu model** (`infra/tofu/variables.tf`
validates only 3 dom0 keys — a known IaC gap). Canonical roster (4 dom0s / 9 heavy
slots): `install-helpers/farm-topology.sh`.

## Architecture (two-level elastic scaler)

```
worklist @farm queue ──▶ FARM-AUTO reconciler (timer, no AI)
                          └─ farm-autoscaler step:
   1. read pending jobs → classify each: BIG (whole-workspace/release) | SMALL (per-crate / agent pod)
   2. per dom0, choose ONE shape (L4 mutual exclusion):  big | small×N | off
   3. write infra/tofu shape vars → `tofu apply`  (clone from MDE-VM-golden, scale-to-zero idle)
   4. between jobs: `xe snapshot-revert` the VM to its clean post-toolchain snapshot (L3)
   5. dispatch the job to the provisioned VM; POD level: cap podman build/agent pods per VM to its vCPU/RAM
```

- **The queue is the only signal (L1).** A job's shape is inferred from its kind: a
  `cargo build --workspace --release` / RPM cut = **BIG** (wants a whole dom0); a
  `cargo build -p <crate>` or an agent pod = **SMALL**. The autoscaler counts the
  pending big-vs-small mix per target dom0.
- **VM level (Xen, per dom0):** tofu's `build-vms.tf` computes the VM set from a
  per-dom0 `shape` var (`big` / `small` / `off`) + `small_count`. `tofu apply`
  converges; idle → `off` (scale-to-zero). VMs clone `MDE-VM-golden` (XCP-2) +
  ansible toolchain (already in `infra/`).
- **POD level (podman, per VM):** within a small VM, agent/build pods scale up to
  the VM's core/RAM budget; the autoscaler folds pod demand into the shape choice
  (many pods queued for one dom0 → small×N rather than one big).
- **Mutual exclusion + drain (L4):** switching a dom0's shape first **drains** the
  current jobs (lets them finish), then `tofu destroy` the old shape and `apply` the
  new. Hysteresis/debounce (a min dwell time) prevents thrashing against tofu's
  ~minutes apply latency.
- **Snapshot-reset (L3):** each freshly-toolchained build VM gets a `clean`
  snapshot; inter-job reset is a fast `xe snapshot-revert` rather than a re-clone —
  clean state without the full clone/boot cost. Cargo's `target/` persists *within*
  a job's VM (warm incremental); reset returns to the golden baseline.

## Acceptance (each runtime-observable)
- A queued whole-workspace/release build → autoscaler tofu-provisions the **big** VM
  on its dom0 (≈ all the hardware), runs it, then scales it down.
- Many queued per-crate builds / agent pods → autoscaler provisions **several small**
  VMs (spread across dom0s), runs them in parallel, scales to **zero** after.
- A dom0 is **never** running big + small simultaneously (mutual exclusion verified).
- Idle farm → **zero** build VMs running (scale-to-zero; the dom0s sit free).
- **Zero AI tokens** spent in the autoscale loop (the reconciler drives tofu).
- Inter-job VM state is the clean golden baseline (snapshot-revert verified).

## Risks
- **tofu apply/destroy latency (~minutes)** vs. build duration → hysteresis + a
  min-dwell so a dom0 doesn't flap shapes; batch the apply per tick.
- **Snapshot-revert correctness** — the `clean` baseline must carry the toolchain +
  sccache priming but no job state; verify on a reset drill.
- **SR / disk pressure** on the dom0s from clones + snapshots → cap concurrent VMs
  per dom0 to its SR headroom; the disk-watchdog class applies on-VM too.
- **XO (Xen Orchestra) API availability** for tofu apply — degrade to the last-good
  topology if XO is unreachable; never strand a running build.
- **Thrash under a mixed queue** (big + small interleaved) → priority rule: a big
  build waits for a min batch of smalls to drain, and vice-versa, rather than
  flipping every tick.

## Out of scope
- Cross-dom0 live migration (XCP-ng pools are per-dom0 here).
- Replacing the operator-gated `/release` cut.
- Public cloud burst (DO/lighthouse infra stays separate).
