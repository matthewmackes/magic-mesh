# XCP-ng dom0 full-mesh-peer + native XAPI provisioning

Status: **locked** (operator survey 2026-06-20, 2 rounds). Owner epic prefix:
`XCP` (extends [`xcp-ng-integration.md`](xcp-ng-integration.md), XCP-1..7).
Reference: <https://xcp-ng.org/> · XO REST API:
<https://docs.xcp-ng.org/management/manage-at-scale/xo-api/>.

## What this changes

`xcp-ng-integration.md` (XCP-5/6, corrected 2026-06-19) retired "native mackesd
on dom0": an xcp-ng **dom0 is CentOS-7 (glibc 2.17)** and the Fedora `magic-mesh`
RPM can't run there, so a dom0 today is only an **overlay member** (the static
nebula binary) driven remotely via `XeSsh`. This epic makes a dom0 a **full mesh
peer** — network **+ monitoring + alerting + provisioning**, running **on the
dom0** — via a purpose-built static agent shipped as an RPM the Workbench
installs remotely. It also re-backs the Provisioning tab on a **native rustls
XAPI** client (replacing `xe`-over-SSH as the primary control plane) and stands
up **xo-server** as a complementary mesh service.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| P1 | dom0 agent runtime | **Slim static (musl) agent** — a new minimal Rust binary (`mde-xcp-agent`), `x86_64-unknown-linux-musl`, statically linked → runs on any el7 dom0 with **zero glibc dependency**. Minimal dom0 footprint (dom0 best practice). NOT a full-mackesd port. |
| P2 | Install | **RPM pushed over the overlay/SSH from the Workbench** — a `mackesd`/Workbench verb pushes the `.rpm` to the dom0 over Nebula+SSH and runs `rpm -Uvh` + enables the units. (Supplemental-pack packaging is a hardening follow-on.) |
| P3 | Build/test | **Throwaway nested xcp-ng VM** — the musl-static agent builds on a Fedora slot (no el7 build env needed — static); install/test on a disposable nested-virt xcp-ng VM as a fake dom0. Production hosts untouched. |
| P4 | Scope | **All four in one RPM** — network + monitoring + alerting + provisioning ship together in `mde-xcp-agent`. |
| P5 | Tab backend | **Native rustls XAPI** — `mackes-xcp` gains a `Xapi` backend (JSON-RPC/XML-RPC to each host `:443` over **rustls**, no OpenSSL — §3); it becomes the Provisioning tab's primary control plane. `XeSsh` stays for host bootstrap + the agent-install path. |
| P6 | Orchestration | **Native Rust XO *replacement* (`mde-orchestra`)** — operator override 2026-06-20: do NOT deploy community `xo-server`. Build a full Xen-Orchestra-equivalent in Rust on top of the P5 rustls XAPI client, integrated into the Workbench as the **first-class provider of virtual services**. No Node.js / no `xo-server` dependency. (Earlier "xo-server on a lighthouse" lock retired.) |
| P7 | Auth | **Shared service account** — one XAPI service credential, encrypted on the mesh secret plane (leader-managed, like XCP-7); absent from `ps`/logs. |

## Architecture

```
crates/mesh/mde-xcp-agent/          NEW — the dom0-resident peer agent (musl static)
  network    : manage the static nebula (config/cert/service) — reuse XCP-5's proven method
  monitor    : sample host health (CPU/RAM/SR free/temps/running VMs) via xcp-rrdd + xe
  alert      : emit dom0 events (SR full, host degraded, VM crash, patch available) to the mesh
  provision  : receive typed provisioning verbs (spawn/clone/destroy/migrate) and drive local xe
  transport  : talk the mesh over the overlay (a slim bus client / signed verbs) — NO mackesd dep
packaging/xcp-agent/                 NEW — the el7 RPM (spec + units), built from the musl binary
  mde-xcp-agent.service + nebula.service + a monitor/alert timer
crates/mesh/mackes-xcp/              EXTEND — add the native rustls XAPI backend
  trait Hypervisor (exists)          clone/start/destroy/vm_ip/list/host_capacity
  impl XeSsh (exists)                bootstrap + agent-install path
  impl Xapi (NEW, P5)                rustls JSON-RPC to host :443; the tab's primary backend
mackesd:
  ipc/provision.rs (extend)          action/provision/* now over Xapi; + dom0-agent install verb
  workers/xcp_host.rs (revise)       consume the dom0 agent's published capacity/health/alerts
mde-workbench:
  panels/provisioning/*              full VM/host/pool/SR/network ops via the Xapi backend
services (XO):
  install-helpers/setup-xo-server.sh NEW — deploy community xo-server on a lighthouse (P6)
secrets:
  <Mesh-Sync>/secrets/xcp/service.age (P7) leader-managed XAPI/XO shared creds
install:
  install-helpers/xcp-agent-install.sh  push+install the RPM on a dom0 over the overlay (P2)
```

### dom0 agent — the four functions (P4, all in `mde-xcp-agent`)
1. **Network** — own the static nebula on dom0 (XCP-5's proven static-binary +
   systemd method); re-assert on boot. The dom0 is a first-class overlay member.
2. **Monitoring** — sample host health from `xcp-rrdd` + `xe host-data`/`sr-list`
   and publish to the mesh (the directory + a metrics topic); the dom0 shows up
   like any peer in health rollups.
3. **Alerting** — watch dom0 conditions (SR ≥ threshold, host degraded, VM
   crash/HA event, available patches) and emit mesh alerts (the §EFF alert path),
   journald-first so a headless dom0 still surfaces them.
4. **Provisioning** — accept **typed, signed** provisioning verbs over the overlay
   (no raw shell channel — §9 W21/W32) and execute them via local `xe`: spawn/
   clone/destroy/migrate VMs, manage SRs/networks. The dom0 is a compute provider
   that *accepts* work, not just advertises it.

### Native XAPI backend (P5)
The Provisioning tab drives every reachable host directly through a Rust rustls
XAPI client (`Xapi` impl of `Hypervisor`): session login (shared service account,
P7), then the XenAPI calls for VM/host/pool/SR/network lifecycle. No `xe`
subprocess, no OpenSSL. `XeSsh` is retained only for first-contact bootstrap
(before the agent/overlay exists) and the agent-install push.

### xo-server mesh service (P6)
A community `xo-server` runs on a lighthouse as a mesh service for the
heavyweight XO capabilities not worth reimplementing (delta backups, replication,
the rich web console). Available to operators; the tab does **not** depend on it.

## Build + test (P3)
- **Build:** add the `x86_64-unknown-linux-musl` target on a Fedora build slot;
  `cargo build -p mde-xcp-agent --target …-musl --release` → a static binary.
  Package the el7 RPM (the static binary + units + nebula static) — the RPM is
  essentially file-drop + scriptlets (no compiled deps to satisfy on el7).
- **Test:** provision a **nested-virt xcp-ng VM** (a real xcp-ng ISO install in a
  VM, nested virtualization on) as a disposable dom0; push+install the RPM via the
  Workbench path; verify the four functions; destroy/recreate at will. No
  production host touched.

## Acceptance (each runtime-observable, §7)
- A nested xcp-ng "dom0" with the RPM installed appears in the mesh **directory**
  as a peer (not just pingable) with role/capability tags.
- Its **host health** (CPU/RAM/SR/VMs) shows in the Workbench fleet rollup.
- A forced dom0 condition (e.g. SR > threshold) raises a **mesh alert** visible in
  the Action Center / journal.
- A **provision verb from the Workbench** (spawn an MDE-VM) executes on that dom0
  via the agent and the VM joins the mesh.
- The Provisioning tab performs VM/host/SR/network ops via the **native rustls
  XAPI** backend (no `xe` subprocess in the hot path) against a live host.
- `xo-server` reachable as a mesh service; the shared service account drives both.
- The agent **survives a dom0 reboot** (units re-assert) and a host **patch/upgrade
  cycle** (documented re-assert, since dom0 upgrades can wipe non-pack files).

## Risks / mitigations
- **dom0 fragility / host-upgrade survival** — non-supplemental-pack files can be
  lost on a host upgrade; mitigate with an idempotent re-assert unit + document
  the supplemental-pack follow-on (P2 note).
- **musl static + `xe`/xcp-rrdd** — the agent shells dom0 tools; ensure they exist
  on every supported xcp-ng (8.2/8.3) and degrade gracefully.
- **XAPI scope creep** — the native rustls XAPI client is large; implement the
  verbs the tab needs first, not the whole XenAPI.
- **Security** — provisioning verbs are typed + signed (no raw shell); the shared
  service account is least-privilege where XAPI RBAC allows; creds never in `ps`.
- **Nested-virt test fidelity** — nested xcp-ng may differ subtly from bare metal;
  a final check on a real throwaway dom0 before declaring §7-done.

## Out of scope (this epic)
- Full Xen Orchestra feature parity in the native backend (XO covers the rest).
- dom0 GUI (dom0 is headless; the Workbench is the surface).
- Windows/other-hypervisor targets.
