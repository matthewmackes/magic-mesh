# DevOps "Backoffice" rebuild — reproducible + portable to a new Nebula (DEVOPS-AUTOMATION-REBUILD)

**Status:** ⏸ **SURVEY IN PROGRESS — paused at Q12/100** (operator switched back to `/ship`).
Resume the survey at **Round 4 / Q13** (DR, backend reachability, per-mesh config,
reconstituting the current setup) and continue through ~Q100, then finalize this
doc + lift into `docs/WORKLIST.md` as the `DEVOPS-*` epic.

**Vision (operator):** "Rebuilding, or building, the DevOps Backoffice. Reconstitute
Tofu and the rest of the backoffice components — an important option / part of
building a new Nebula. When creating a new XCP-NG MCNF machine, those services need
to come along. What is the setup and deployment process for these parts of the mesh."

## Current state (research, 2026-06-27)
The backoffice today is hand-stood-up on the LAN control node (`172.20.145.192`,
rocky9-kvm2) and is **not** part of any reproducible new-mesh deployment:
- `automation/state-backend/` — etcd-backed Tofu **http state backend** (`state-backend-up.sh`,
  `tofu-state-etcd.py`) at `http://172.20.145.192:8390/state/<root>` (SUBSTRATE-V2).
- `automation/forgejo/` — self-hosted git + CI runner (`forgejo-runner-up.sh`).
- `automation/reconciler/` — the FARM-AUTOSCALE reconcile loop (FA_APPLY-safe).
- `automation/secrets/`, `automation/dr/`, `automation/queue/`, `automation/cache/`, `automation/lib/`, `automation/testbed/`.
- Tofu roots: `infra/tofu/` (build-vms farm), `zone1-do` (DO droplets/lighthouses/asterisk),
  `xen-xapi` (the Xen dom0 pool, per-dom0 aliased providers), `edgeos` (gateway DHCP/FW/NAT/VPN).
- `infra/ansible/` — build-VM toolchain + sccache. Build farm = `.50/.90/.130`.

## Locks so far (Q1–Q12, survey 2026-06-27)

| # | Area | Lock |
|---|------|------|
| 1 | Deploy unit | **Dedicated control VM** — one XCP-NG VM per mesh holds state-backend + Forgejo + reconciler + secrets |
| 2 | State bootstrap | **Reuse the mesh etcd** — the founding lighthouse's etcd IS the Tofu state store (no separate backend to bootstrap; `tofu-state-etcd.py` already speaks etcd) |
| 3 | Genesis hook | **`found --with-backoffice` flag** — opt-in at genesis time |
| 4 | Optionality | **Tiered** (Minimal vs Full) |
| 5 | Control VM host | **Founding XCP-NG dom0** — same host that founds the mesh hosts the control VM |
| 6 | Mesh membership | **Full mesh peer** — enrolled, overlay IP; secrets + state are overlay services |
| 7 | etcd isolation | **Same quorum, separate key prefix** (e.g. `/tofu-state/*`) — one cluster; mesh control-plane keys stay separate |
| 8 | Secrets bootstrap | **Mesh secret store, unsealed on deploy** — sealed in the existing store; control VM unseals on enroll |
| 9 | Tier contents | **Min = state-backend + secrets + Tofu roots** (can apply infra); **Full = + CI (Forgejo) + reconciler/autoscaler + build farm + DR** |
| 10 | CI / git | **Self-host Forgejo + runner on the control VM** (Full tier) — sovereign, air-gappable |
| 11 | Build farm | **Backoffice provisions it** (Full tier) — the reconciler/Tofu stands up the build-farm VMs (xen-xapi + ansible toolchain); the farm travels with the mesh |
| 12 | Reconciler | **Continuous loop, systemd-managed** on the control VM (FA_APPLY-safe), like the live autoscaler |

## Remaining survey (Q13–Q100, not yet asked)
Topic backlog to drive when resumed: DR of backoffice state (Q13, leaning: extend the
DATACENTER-23 off-fleet age-push) · state-backend bind reachability (Q14) · per-mesh
config source (Q15, leaning: generated at `found` time) · reconstitute-vs-document the
current hand-built backoffice (Q16) · then: bring-up ordering & dependency graph · XCP-NG
VM template/cloud-init/sizing · Forgejo data model + runner registration + repo seeding ·
sccache topology on a new farm · idempotency/drift-detection of the reconcile · provider
credentials lifecycle (DO/XAPI/Forgejo tokens) rotation · the `--with-backoffice` UX in the
DC-18 genesis wizard · teardown/`backoffice down` · observability of the backoffice itself ·
upgrade path of the control VM · air-gap/offline-first bundling · multi-mesh isolation ·
state migration (`tofu state push`) for the live mesh · how `xen-xapi` learns a new dom0 ·
edgeos/gateway-as-code on a new site · testbed/L1-L3 on a new mesh · etc.
