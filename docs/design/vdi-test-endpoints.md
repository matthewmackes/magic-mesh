# TESTVM — Spice / VNC / RDP test endpoints for the VDI bed

Operator-locked 2026-07-03 (`/plan`, 1-round survey). A **series of small throwaway
XCP-ng VMs** that each expose a remote-desktop endpoint (Spice, VNC, RDP) so the
"Construct" shell's **Desktop / VDI surface** (the desktop chooser + the ironrdp / VNC /
spice viewers, E12-4/5/6/7 + OW-8) has real targets to connect to. **Only basic
connection is required** — the guests just need to accept a session; no rich desktop,
no persistence, no auth hardening.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Host / hypervisor | **XCP-ng VMs on a dom0** — native to the farm, `xe` / tofu-managed (`infra/tofu/xen-xapi`). VNC is free via the XAPI console; Spice + RDP are in-guest servers. |
| 2 | Layout | **2 VMs** — a **Linux** guest serving **Spice + VNC**, and a **Windows** guest serving **RDP** (authentic Windows RDP, not xrdp). |
| 3 | Linux guest OS | **Minimal Alpine** — tiny, fast-booting; runs the Spice server (`spice-vdagent` + a QEMU/XAPI spice channel or `Xspice`) + `x11vnc`. |
| 4 | Reachability | **dom0 LAN bridge** — endpoints live on `172.20.x`, reached by `IP:port`. Simplest path for a pure "does it connect" check; no Nebula overlay for v1. |

## Endpoints (what each VM exposes)

| VM | OS | Endpoint(s) | How served | Default port |
|----|----|-------------|-----------|--------------|
| `testvm-lin` | Alpine | **VNC** | `x11vnc` on a minimal X (or the XAPI console VNC) | 5900 |
| `testvm-lin` | Alpine | **Spice** | Spice server (Xspice / spice channel) | 5930 |
| `testvm-win` | Windows | **RDP** | native Windows Remote Desktop (`mstsc`-compatible) | 3389 |

The shell connects via its existing viewers: RDP → `mde-vdi-rdp` (ironrdp), VNC →
`mde-vdi-vnc`, Spice → the spice viewer path. "Basic connection" DoD = each viewer
opens the endpoint and shows the guest's framebuffer / login.

## Architecture

- **Provisioning:** an `xe`/tofu bringup on a chosen dom0 (default a farm dom0 with
  headroom — `.193 KVM-XCP1` or `.9 XEN-HOME-SERVICES`; pick by live capacity at
  bringup). Reuse `infra/tofu/xen-xapi` where it fits; a thin `install-helpers`
  bring-up script is acceptable for throwaway VMs (they are not fleet infra).
- **Linux guest (`testvm-lin`):** boot a **locally-mirrored Alpine** ISO/image (airgap
  — must be reachable from the build env; if absent, mirror it first). Cloud-init /
  an answer file installs `x11vnc` + a spice server + a minimal X, opens 5900/5930 on
  the LAN, no password (or a trivial documented one) — basic connection only.
- **Windows guest (`testvm-win`):** boot a **locally-available Windows ISO/template**
  and enable Remote Desktop (3389). ⚠️ **OPEN ITEM:** the airgapped farm may not have a
  Windows image — if none exists, **fall back to Alpine + `xrdp`** on 3389 (documented
  degradation; still exercises the ironrdp path) and surface the gap to the operator.
- **Networking:** attach each VM to the dom0's LAN bridge so it gets a `172.20.x`
  address; verify the endpoint port is reachable from the shell host.

## Acceptance (runtime-observable)
- `testvm-lin` boots on the dom0, is reachable at `172.20.x`, and **accepts a VNC
  connection** (a viewer shows its framebuffer) **and a Spice connection**.
- `testvm-win` (or the Alpine+xrdp fallback) boots, is reachable, and **accepts an RDP
  connection** (ironrdp / `mde-vdi-rdp` shows the login/desktop).
- The shell's **Desktop chooser** lists / can be pointed at each endpoint and connects
  (basic session — no rich-desktop requirement).
- The VMs are clearly throwaway (documented teardown: `xe vm-shutdown` + `vm-destroy`).

## Risks / open items
- **Windows image on the airgap** (#2) — no local Windows ISO ⇒ RDP falls back to
  Alpine+xrdp. Confirm with the operator whether a Windows image is available or the
  fallback is acceptable.
- **Spice on XCP-ng** — XCP-ng doesn't serve Spice host-side; it runs *in-guest*
  (Xspice / spice-server). Slightly more guest setup than VNC.
- **Alpine mirror on the airgap** — the Alpine ISO/packages must be locally reachable;
  mirror first if absent.
- **No overlay (#4)** — LAN-only for v1; a later task can join them to Nebula to test
  the production mesh path (out of scope now).

## Out of scope
- Nebula-overlay reachability (v1 is LAN-only).
- Rich/persistent desktops, real auth, multi-user, GPU.
- Fleet management / autoscale of these VMs (throwaway, hand-torn-down).

## Tasks → see `docs/WORKLIST.md` TESTVM-1..4.
