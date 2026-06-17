# XCP-ng Integration — provision MDE-VMs + XCP hosts join the mesh

Status: **locked** (operator survey 2026-06-16). Owner epic prefix: `XCP`.
Community/back-reference: <https://xcp-ng.org/#community>.

Magic Mesh gains first-class XCP-ng integration in two halves that share one
hypervisor-access layer:

- **A — Provisioning:** the platform spins up headless **Server**-role MDE-VMs on
  an XCP-ng host (local *or* remote) and auto-joins them to the mesh.
- **B — Host join:** an XCP-ng host itself becomes a mesh member and advertises
  its compute capacity, so the XCP fleet is a distributed compute plane.

## Locked decisions (survey)

| # | Decision | Lock |
|---|----------|------|
| A1 | Hypervisor access | **xe-over-SSH** — mackesd SSHes to dom0 and drives `xe`; identical path local + remote (host + creds). Glue, not reimplementation (§6). A `Hypervisor` trait leaves room for a native rustls XAPI backend later. |
| A2 | Base image | **XCP template `MDE-VM-golden`** — built once (F44 cloud + UEFI + cloud-init, generalized: no host keys/machine-id); each spawn is a fast `xe vm-clone`. |
| A3 | Mesh join | **Auto-enroll join** — provisioning runs `network-enroll join` so the node boots already a Server in the directory. |
| A4 | Surface | **CLI + bus + panel** — `mackesd provision …`, `action/provision/*`, and a Workbench panel under the Provisioning plane (MESH: VIRTUAL WORKLOADS). |
| B1 | Host join model | **Native on dom0** — nebula + lizardfs-client + mackesd installed on dom0; the hypervisor *is* the node. (Appliance-fragility mitigations below.) |
| B2 | Host role | **Compute provider** — host advertises CPU/RAM/SRs/running-VMs into the directory; any node can target it to spawn MDE-VMs. |
| B3 | Credentials | **Mesh secret on QNM-Shared** — XAPI/host creds encrypted on the replicated LizardFS plane (leader-managed, like the CA backup). |

## Conventions (hard rules, all spawns)

- **Hostname** of every created machine starts with **`MDE-VM`** (operator rule
  2026-06-16): `MDE-VM-<name>`.
- Fresh identity per clone: regenerated SSH **host** keys + `machine-id`, the
  operator's authorized key injected (cloud-init `ssh_deletekeys` + a
  `machine-id` reset on first boot via a new instance-id seed).
- **UEFI required** (`HVM-boot-params:firmware=uefi`, `secureboot=false`) — Fedora
  Cloud Generic images do not boot under SeaBIOS (proven 2026-06-16).
- Headless **Server** role: `mackesd role-pin server`; no Cosmic desktop.
- Reuse the proven recipe (see `[[xcp-ng-test-host]]` memory): SR mount staging,
  `xe vdi-import`, VIF on `xenbr0`, IP via `tcpdump` on the VIF MAC.

## Architecture

```
crates/mesh/mackes-xcp/         NEW — the hypervisor-access layer
  trait Hypervisor { clone_golden, set_identity_seed, start, vm_ip, list, destroy, host_capacity }
  impl XeSsh (A1)               ssh + xe; creds from the mesh secret (B3)
mackesd:
  ipc/provision.rs              action/provision/{spawn,list,destroy,hosts}
  workers/xcp_host.rs           (B) on dom0: advertise host capacity to the directory
  cmd: `mackesd provision …`    CLI surface (A4)
mde-workbench:
  panels/provisioning/vm_spawner.rs   (A4) GUI: spawn/list/destroy MDE-VMs, pick host
secrets:
  <QNM-Shared>/secrets/xcp/<host>.age (B3) leader-managed XAPI creds
install:
  install-helpers/xcp-host-join.sh    (B) native dom0 nebula+lizardfs+mackesd (appliance-guarded)
  install-helpers/build-mde-vm-golden.sh  (A2) one-time golden template builder
```

### A — provisioning flow
1. Resolve target host (default local) + creds (mesh secret, B3).
2. `xe vm-clone MDE-VM-golden → MDE-VM-<name>`; attach a fresh cloud-init seed
   (new instance-id, hostname `MDE-VM-<name>`, op key, regen host keys + machine-id).
3. Start (UEFI inherited); resolve IP via `tcpdump` VIF MAC.
4. Over SSH: `dnf -y upgrade`; `mackesd role-pin server`; `network-enroll join` (A3).
5. Record the VM in the directory under the owning host's capacity rollup.

### B — host-join flow (native dom0, appliance-guarded)
1. `xcp-host-join.sh` installs nebula + lizardfs-client + mackesd on dom0 from a
   pinned bundle; idempotent; **re-asserted by a boot unit** so an XCP host
   upgrade that clobbers it self-heals (mitigates the dom0-appliance risk).
2. dom0 pins **Server** role, joins the overlay + QNM-Shared, runs `xcp_host`
   worker → publishes capacity (CPU/RAM/SR free/running VMs) to the directory.
3. The Provisioning panel/`action/provision/hosts` lists every joined host as a
   spawn target (B2 compute plane).

## Acceptance (runtime-observable)
- `mackesd provision spawn --name web1 [--host H]` → a booted `MDE-VM-web1`,
  Server role, in the mesh directory within the join window; SSH-reachable with
  the op key; **hostname starts `MDE-VM`**.
- `action/provision/list` + the panel show live MDE-VMs; `destroy` removes one.
- A joined XCP host appears in the directory with live capacity; spawning with
  `--host <that host>` places the VM there.
- XAPI creds never appear in `ps`/logs (mesh-secret, B3).
- Re-run safety: a second `xcp-host-join` is a no-op; an XCP host reboot
  re-asserts the dom0 platform bits.

## Risks / out of scope
- **dom0 fragility (B1):** native installs on a locked appliance can break on
  host upgrades — mitigated by the idempotent re-assert boot unit; revisit a
  per-host agent VM if it proves unstable.
- Native rustls XAPI backend (replacing xe-over-SSH) — deferred behind the trait.
- Windows/other guest images, live-migration orchestration — out of scope.
