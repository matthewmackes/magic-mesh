# XCP-ng Integration — provision MDE-VMs + XCP hosts join the mesh

Status: **locked** (operator survey 2026-06-16). Owner epic prefix: `XCP`.
Community/back-reference: <https://xcp-ng.org/#community>.

> **Note (post-SUBSTRATE-6):** the B1 lock and host-join flow below describe a dom0
> joining with **nebula + lizardfs-client + mackesd**. **LizardFS is removed** — the
> substrate is now **etcd** (coordination) + **Syncthing** (files). The "Tested
> onboarding (2026-06-19)" section already corrects B1 (mackesd/lizardfs-client
> can't run on a CentOS-7 dom0 anyway; the dom0 is driven via `XeSsh`); etcd +
> Syncthing both ship as static binaries, which is what could later let a dom0 carry
> a real coordination/file agent. Read every `lizardfs-client` mention here as the
> retired LizardFS plane.

MCNF gains first-class XCP-ng integration in two halves that share one
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
| B1 | Host join model | **Native on dom0** — nebula + (the substrate client) + mackesd installed on dom0; the hypervisor *is* the node. (Originally `lizardfs-client`, retired by SUBSTRATE-6 → etcd/Syncthing; further corrected in the 2026-06-19 onboarding section — mackesd/lizardfs can't run on a CentOS-7 dom0, so it's driven via `XeSsh`.) |
| B2 | Host role | **Compute provider** — host advertises CPU/RAM/SRs/running-VMs into the directory; any node can target it to spawn MDE-VMs. |
| B3 | Credentials | **Mesh secret on the shared mesh-storage plane** — XAPI/host creds encrypted on the replicated `/mnt/mesh-storage` dir (now Syncthing-synced, was the LizardFS plane), leader-managed, like the CA backup. |

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
  install-helpers/xcp-host-join.sh    (B) native dom0 nebula (+ static etcd/Syncthing later; lizardfs retired SUBSTRATE-6); driven via XeSsh (appliance-guarded)
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
1. `xcp-host-join.sh` installs nebula on dom0 from a pinned bundle (originally
   "+ lizardfs-client + mackesd" — LizardFS retired by SUBSTRATE-6, and per the
   2026-06-19 correction mackesd doesn't run on dom0 at all; see below);
   idempotent; **re-asserted by a boot unit** so an XCP host upgrade that clobbers
   it self-heals (mitigates the dom0-appliance risk).
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

## Tested onboarding method (2026-06-19) — and a B1 correction

Designed + tested end-to-end against a real XCP-ng 8.3.0 dom0 (`172.20.0.4`) via a
**self-contained throwaway CA + test lighthouse** on the dev box (overlay `10.99.0.0/24`,
port 4299, tun `nebula9` — isolated from the production `nebula1`/`10.42` mesh).
Script: `install-helpers/onboard-xcp-host.sh`. **Result: dom0 joined the test
overlay as `10.99.0.20` with 0% loss bidirectional to the lighthouse over Nebula,
then fully torn down; production mesh untouched.**

**The method (proven):** mint a node cert on the CA → push a **statically-linked**
`nebula` + ca/host certs + a `config.yml` (static_host_map → lighthouse public IP,
non-lighthouse, open-mesh fw) to dom0 → install `nebula.service` (systemd, the right
persistence — a detached `nohup`/`setsid` over SSH does **not** survive) → verify
overlay ping.

**B1 CORRECTION (load-bearing).** The original B1 lock — "native nebula +
**lizardfs-client + mackesd** on dom0" — is **only partly viable** (and the
`lizardfs-client` half is moot anyway: LizardFS is retired by SUBSTRATE-6). An
XCP-ng dom0 is CentOS-7-based (**glibc 2.17**); the Fedora-packaged `nebula`,
`mackesd`, and the old lizardfs client are **dynamically linked against glibc
≥2.32/2.34** and die on dom0 with `version 'GLIBC_2.34' not found` (verified).
Therefore:
- **Nebula on dom0: YES**, but ONLY the **official SlackHQ static** release
  (`nebula-linux-amd64.tar.gz`, statically linked Go) — NOT the Fedora `/usr/bin/nebula`.
  The script now downloads/uses the static binary and **refuses to push a dynamic one**.
- **mackesd on dom0: NO** (glibc); the retired `lizardfs-client` likewise. The
  dom0 does **not** run mackesd. Drive it instead via the **A1 `XeSsh` path**
  (`HostTarget::Ssh` over the
  overlay) from a real mesh node, and run the **`xcp_host` capacity worker on a
  designated Server node (or the leader)** pointed at the dom0 over SSH — NOT on dom0
  locally. (This supersedes the XCP-6 "xcp_host self-gates on the dom0 marker and runs
  *on* dom0" assumption — mackesd can't run there.) The 11.0 substrate move to
  **etcd + Syncthing** (both shippable as static binaries) is what could later let a
  dom0 carry a real coordination/file agent without the glibc wall.

So a dom0's mesh membership = **overlay reachability + SSH-over-mesh + XAPI control
target**, which is exactly enough for compute-provider duty (A-plane spawns target it
via `xe`). "Full native mackesd member" is retired for dom0.

## Risks / out of scope
- **dom0 fragility (B1):** the static `nebula` + its `nebula.service` are the only
  dom0-resident bits; an XCP host upgrade clobbering them self-heals via the
  idempotent re-assert (a boot unit / re-run is a no-op). Far smaller surface than the
  original native-mackesd plan.
- **dom0 glibc wall:** no Fedora RPM (mackesd; and the retired lizardfs) runs on
  dom0 — drive via `XeSsh`; only static binaries (nebula today; etcd/Syncthing under
  SUBSTRATE-V2, now the live substrate) are dom0-resident candidates.
- Native rustls XAPI backend (replacing xe-over-SSH) — deferred behind the trait.
- Windows/other guest images, live-migration orchestration — out of scope.
