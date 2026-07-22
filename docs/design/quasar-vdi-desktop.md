# Construct — the egui-native mesh thin-client desktop OS (E12, revised)

> **Design-standard note (2026-07-22):** look-and-feel guidance in this doc is subordinate to the platform interface standard — see [platform-interfaces.md](platform-interfaces.md) (Apple-HIG-principled Construct + Car). Feature/behavior content remains authoritative.

> **Status:** LOCKED (design) · 2026-06-30 · 50-question `/plan` survey.
> **Series:** MCNF **12.0 "Construct"** (package/repo id stays `magic-mesh`).
> **Authority:** Memory > `AI_GOVERNANCE.md` > this doc > `docs/WORKLIST.md` body.
> **Supersedes (in part):** `docs/design/cosmic-magic-mesh-egui.md` — the
> **forked-compositor** desktop model (its locks 1/5/6/12: fork `cosmic-comp`,
> compositor-up desktop, two-bucket boundary, mesh-aware compositor) is
> **retired**. What carries forward from that doc: **egui as the one toolkit**
> (lock 2), the **fresh egui-native design / `Style` source** (locks 3/9), and
> **egui built-in motion** (lock 10) — i.e. the `mde-egui` harness (E12-1, landed).
>
> **Revision 2026-07-03 — CONSTRUCT-CLOUD supersession.** `docs/design/quasar-cloud.md`
> and `AI_GOVERNANCE.md §5` replace the local cloud-hypervisor target with
> **Nova/libvirt/QEMU-KVM + OVN**. The cloud-hypervisor/`mde-kvm` text below is
> preserved as the 2026-06-30 survey record and cutover context only; current
> implementation and packaging work targets libvirt/QEMU-KVM, and QC-15 owns
> deletion of the replaced stack.

## What this is

MCNF 12.0 is a **mesh-native thin-client desktop OS** whose entire interface is
**egui**. The host is not a general Linux desktop that runs native apps — it is a
single egui shell that **brokers and displays full OS desktops** (Windows / Ubuntu
/ etc.) running either **locally on Nova/libvirt/QEMU-KVM** or **remotely on any
mesh peer**, reached over the Nebula overlay.

The decisive insight: **a web browser, office suite, or game is never a host app —
it runs inside a VM guest.** That dissolves the "do we need a Wayland compositor to
host a browser" question — the host stays pure egui, and the heavy third-party
software lives in a guest desktop the shell renders as an egui texture.

Two roles, one shell: **Lighthouse · Workstation** all run the *same* stack.
Headless Workstations serve VM/container capacity without starting the egui seat;
Workstations with a display add the local DRM seat + VDI layer. There is exactly
**one shell to build**; VDI is a capability layered onto it. XCP-ng is day-2
adopted capacity, never an install-time role.

## Locked decisions (the 50-question survey, 2026-06-30)

### Round 1 — Shell & session model
| # | Decision | Lock |
|---|---|---|
| 1 | VM presentation | **Fullscreen, one desktop at a time** (not tiled/windowed). |
| 2 | Shell ⇄ VM switch | A **thin persistent egui chrome bar** (peers · sessions · status) frames the fullscreen VM. |
| 3 | Input grab | **Always passthrough** to the focused guest; one **reserved escape chord** returns to the shell. |
| 4 | Clipboard | **Bidirectional host⇄guest, integrated with mesh clipboard-sync** (flows across peers + VMs). |
| 5 | Audio | **virtio-sound** for local VMs (low latency) + the **protocol channel** (RDP/VNC) for remote. |
| 6 | File transfer | A **mesh-synced share folder** (Syncthing on host → virtio-fs/redirect into the guest). |
| 7 | Workspaces | **A workspace *is* a session** — switching workspaces switches desktops (folds E12 lock 12). |
| 8 | Multi-monitor | **Operator-configurable** per-monitor mapping (a VM can span all, or one-per-monitor). |
| 9 | Workbench | The thin bar **expands into the full mesh-control Workbench** on demand, then collapses. |
| 10 | Session state | **Persisted per-peer in mesh state** (etcd/Syncthing) — open sessions + layout **roam** to any Workstation. |

### Round 2 — Local KVM (Workstation-local VMs)
| # | Decision | Lock |
|---|---|---|
| 11 | VMM | **Superseded by CONSTRUCT-CLOUD:** Nova/libvirt/QEMU-KVM + OVN replace the original cloud-hypervisor target. |
| 12 | Local display | **virtio-gpu zero-copy** (dmabuf → wgpu texture) fast path; RDP/VNC fallback. |
| 13 | GPU | **Operator choice per host** — shared virtio-gpu (virgl/venus) *or* dGPU passthrough (VFIO). |
| 14 | Guest images | **Mesh-distributed golden images** (Syncthing) + operator ISOs. |
| 15 | Windows tooling | Golden Windows image **pre-tooled**: virtio drivers (net/disk/gpu) + RDP enabled + guest agent. |
| 16 | Lifecycle UI | **Extend the existing DATACENTER/Instances surface** to local libvirt/Nova instances (one VM UI, local + remote). |
| 17 | Resources | **Reserved fixed slice for the shell** + operator-capped per VM. |
| 18 | Disk storage | **Running disks local** (`~/Local`, never mesh-synced, §1); only golden bases are mesh-distributed. |
| 19 | Guest network | **Dual-homed: every VM is its own Nebula mesh peer AND has a virtual NIC on the host's LAN.** |
| 20 | Local vs remote | **Operator chooses**, with a smart default (heavy/GPU → local; always-on → remote peer). |

### Round 3 — Remote desktops over the mesh
| # | Decision | Lock |
|---|---|---|
| 21 | Protocol | **RDP primary** (ironrdp into the guest), **VNC/XAPI console fallback** (universal, any guest state). |
| 22 | Discovery | **Mesh service registry + DATACENTER inventory** — aggregated mesh-wide desktop list. |
| 23 | Auth | **Mesh cert gates the connection** (§8 flat trust / W1); guest-OS login still applies; optional sealed cred vault for auto-login. |
| 24 | Transport | **Direct over Nebula** peer-to-peer; Nebula hole-punch + **auto-relay via a lighthouse** for NAT. |
| 25 | Roaming | **Persistent VM, reconnect from any Workstation** (the live session follows the user). |
| 26 | Codec | **Adaptive to mesh link quality** (H.264/RemoteFX on good links → lighter on weak/relayed). |
| 27 | Lifecycle owner | **The hosting peer owns lifecycle; the leader coordinates;** the Workstation requests via typed mackesd verbs (§9, no push-SSH). |
| 28 | Concurrency | **One active fullscreen session; others connected in the chrome bar**, switchable instantly. |
| 29 | Who serves | **Any Workstation can serve desktops** through the libvirt/QEMU-KVM cloud stack; XCP-ng hosts are optional day-2 adopted capacity. |
| 30 | On disconnect | **VM keeps running** (reconnect later); operator can set a per-VM suspend/shutdown policy. |

### Round 4 — Display & boot foundation
| # | Decision | Lock |
|---|---|---|
| 31 | Compositor | **NONE — the egui shell owns DRM/KMS directly.** No Wayland compositor. |
| 32 | egui→screen | A **winit-less smithay DRM/GBM + libinput runner in `mde-egui`** drives egui+wgpu on a GBM surface. |
| 33 | Native host apps | **None.** Every native app (browser/office/games) runs inside a VM guest. |
| 34 | Boot / seat | **systemd launches the egui shell on the DRM seat** (greetd-style, no display manager); libinput feeds the shell. |
| 35 | Login / lock | **egui login + lock screen**, PAM tied to mesh identity. |
| 36 | cosmic-comp fork | **RETIRED** — replaced by the egui-on-DRM runner + this VDI design (kills the E12-2/9 compositor-maintenance burden). |
| 37 | Surface rewrites | The Workbench/files/music/voice/applet/role-chooser become **panels/modules inside the one shell**, not separate Wayland-client binaries. |
| 38 | Surface scope | **Keep Workbench (control) · Files (mesh browser) · Music (mesh media) · Voice (mesh VoIP) as egui panels** — they're mesh-native features. General computing is in VMs. |
| 39 | Future-proofing | **Commit to egui-on-DRM**; the VM is the escape hatch for any native-app need. Revisit a compositor only on a hard requirement. |
| 40 | GPU baseline | **Vulkan + OpenGL fallback** (wgpu) — broad hardware reach. |

### Round 5 — Identity, packaging, scope, sequencing
| # | Decision | Lock |
|---|---|---|
| 41 | Identity | **Keep E12 "Construct"; revise the design** (this doc + governance §4/§5/§6) to the VDI/egui-DRM model. |
| 42 | Packaging | **Immutable bootc/ostree image** for the Workstation (image-based, appliance-style updates). |
| 43 | Crate structure | **Many small crates** (granular: shell, chrome, each panel, RDP, VNC, KVM-broker, session-broker). |
| 44 | Roles | **Lighthouse · XCP-NG · Workstation** (XCP-NG renamed from Server — the Xen host mirroring the xcp-ng toolstack) **+ a `desktop-host` capability tag** for peers that serve VMs. |
| 45 | VDI control | **mackesd workers** — a session-broker worker + a vm-lifecycle worker. The shell renders; mackesd brokers (§1/§9). |
| 46 | §8 envelope | **Raise the envelope** — VM desktop guests are first-class nodes; the supported node count grows to accommodate them. |
| 47 | Guest security | **Full flat-trust mesh members** — guests are full peers (§0-Simple). The widened blast radius is documented for operators. |
| 48 | v1 scope | **Everything in v1** — GPU passthrough, USB redirection, per-monitor-different-VM, live migration all in the first cut. |
| 49 | First milestone | **The egui-DRM shell + chrome bar showing ONE remote desktop over the mesh** (reusing existing DATACENTER/XCP-ng VMs) — proves the whole loop. |
| 50 | Sequencing | **Foundation-first on the hard deps, then farm fan-out** for the disjoint parts. |

## Resulting architecture

```
                MCNF 12.0 "Construct" — one egui shell, mesh VDI, no compositor
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  desktop-shell (one egui binary on the DRM seat — immutable bootc spin)    │
  │    mde-shell-egui:  thin chrome bar  ⇕expand→  Workbench                   │
  │      session view = a FULLSCREEN VM desktop rendered as an egui texture    │
  │      mesh-native panels:  Workbench · Files · Music · Voice                │
  │    mde-egui:   smithay DRM/GBM + libinput runner · Style · Motion          │
  │    mde-vdi:    ironrdp · vnc-rs → egui texture · virtio-gpu fast path ·    │
  │                clipboard/audio/mesh-share bridges · adaptive codec         │
  │                              │  deps point inward  ▼                       │
  ├──────────────────────────────────────────────────────────────────────────┤
  │  platform-services    mackesd:  session-broker worker · vm-lifecycle       │
  │    local VMs: Nova/libvirt/QEMU-KVM + OVN provider network                 │
  │    remote: typed verbs to the hosting peer; leader coordinates             │
  │                              │  deps point inward  ▼                       │
  ├──────────────────────────────────────────────────────────────────────────┤
  │  mesh-substrate    Nebula overlay · etcd · Syncthing · CA/KDC  (unchanged) │
  └──────────────────────────────────────────────────────────────────────────┘
   lint: a dependency edge pointing OUTWARD (substrate→services, services→shell)
         is a CI failure (the layered-tiers gate carries forward).
```

### Crate plan (many small crates, lock 43)
| Crate | Role |
|---|---|
| `mde-egui` *(landed, E12-1)* | Harness: **+ the smithay DRM/GBM + libinput runner**, the shared `Style`, the Motion table. |
| `mde-shell-egui` | The single shell: chrome bar, session view (fullscreen VM texture), panel host, login/lock, boot-to-seat. |
| `mde-vdi-rdp` | ironrdp → egui texture + input forward (RDP primary). |
| `mde-vdi-vnc` | vnc-rs → egui texture (VNC/XAPI-console fallback). |
| `mde-vdi-core` | Session model, adaptive codec, clipboard/audio/mesh-share bridges, protocol-agnostic surface the shell renders. |
| `mde-kvm` | Superseded local cloud-hypervisor broker; retained only until CONSTRUCT-CLOUD cutover deletes the replaced stack. |
| `mde-panel-*` | The mesh-native panels (Workbench/Files/Music/Voice) as in-shell modules reusing today's non-GUI logic. |
| `mackesd` *(extend)* | `session_broker` + `vm_lifecycle` workers; the `desktop-host` capability. |
| Reuse | `mde-bus`, `mackes-xcp` + the DATACENTER VM verbs (remote lifecycle/console), `mackes-mesh-types`, Nebula/etcd/Syncthing substrate. |
| Retire | the iced/libcosmic GUI crates, `mde-theme` (Carbon), `mde-card`; the cosmic-comp fork is **never created**. |

## Acceptance criteria (epic-level, runtime-observable per §7)

1. The Workstation **boots (bootc image) straight to the egui shell on the DRM seat** — no Wayland compositor, no X — and the shell renders through the shared `Style`.
2. The **thin chrome bar** lists mesh peers + sessions + status and **expands into the full Workbench**; the mesh-control panels (Workbench/Files/Music/Voice) work live over the Bus.
3. A **remote desktop** on another mesh peer (XCP-ng or Server) is **discovered**, connected over **Nebula via RDP (ironrdp)**, **rendered as an egui texture**, and is **interactive** (input forwarded; reserved escape chord returns to the shell). VNC/XAPI fallback works for a guest with no RDP.
4. A **local QEMU/KVM desktop instance** runs through the libvirt/Nova host stack, uses the QEMU display path selected by CONSTRUCT-CLOUD, and its desktop renders in the shell.
5. **Sessions roam:** open sessions + layout persist to mesh state and reappear on a second Workstation; a disconnected VM **keeps running** and reconnects.
6. **Clipboard + the mesh-share folder** move data host⇄guest⇄mesh; **audio** plays (virtio-sound local / protocol remote).
7. `mackesd` runs the **session-broker + vm-lifecycle** workers; remote VM actions go through **typed verbs** to the hosting peer (no push-SSH); the leader coordinates.
8. The **layered-tiers lint** passes and fails on a planted outward edge; **no libcosmic/iced/`mde-theme`** remains (`grep` empty); About/greeter reads `MCNF 12.0 "Construct"`.
9. v1 includes **GPU passthrough, USB redirection, per-monitor-different-VM, and live migration** (lock 48) — each runtime-demonstrated.

## Risks

- **R1 — egui-on-DRM backend.** No winit KMS backend exists; the smithay DRM/GBM + libinput + wgpu/GBM path is real new engineering. *Mitigate:* smithay provides the DRM/GBM/libinput primitives; scope it inside `mde-egui` with a headless-testable seam; it is far less to own than a full compositor fork.
- **R2 — QEMU/KVM display path + passthrough.** CONSTRUCT-CLOUD deliberately moves the local VMM to the better-trodden libvirt/QEMU-KVM stack. *Mitigate:* pre-tooled images (virtio drivers); RDP/VNC/SPICE fallback if the virtio-gpu fast path lags; passthrough is operator-opt per host.
- **R3 — Guests as full flat-trust mesh peers (lock 47) + a raised envelope (lock 46).** A Windows guest on flat trust reaches every peer/service — a large blast radius, and more nodes stress etcd/Nebula. *Mitigate:* document the widened blast radius for operators (extends ENT-12); keep guests behind default-deny inbound; revisit per-service ACLs if the envelope grows materially.
- **R4 — "Everything in v1" (lock 48).** Passthrough + USB + multi-mon + migration in the first cut is a large surface. *Mitigate:* the foundation-first sequence (lock 50) lands the core loop early; advanced features fan out across the farm once the shell + `mde-vdi` exist.
- **R5 — Immutable bootc (lock 42).** A new delivery model vs RPM+kickstart; VM disks + mesh state must live on the writable/state partition, not the immutable image. *Mitigate:* `~/Local` + `/mnt/mesh-storage` are state, the egui shell + stack are the image.
- **R6 — Pure-Rust remote-desktop maturity.** ironrdp is strong; pure-Rust VNC (`vnc-rs`) and especially SPICE are thinner. *Mitigate:* RDP-primary (the rich path); VNC only as the universal fallback; SPICE out of scope unless needed.

## Out of scope (this epic)

- A Wayland compositor of any kind (retired; egui owns DRM) and native host apps.
- SPICE as a primary protocol (RDP-primary / VNC-fallback only).
- Per-service ACLs / zero-trust within the mesh (flat trust holds, §8 — now extended to guests).
- Hyperscale (the envelope is raised for guests but the platform stays workgroup-grade, not multi-tenant).
- Mesh substrate redesign (§1–§3 unchanged; only additive session/desktop-state + the raised node envelope).

## Build sequencing (lock 50 — foundation-first, then farm fan-out)

1. **`mde-egui` DRM runner** — the winit-less smithay DRM/GBM + libinput backend (extends the landed harness).
2. **`mde-shell-egui`** on the DRM seat — chrome bar + the Workbench panel + login/lock + systemd boot-to-seat. *(First visible milestone.)*
3. **`mde-vdi`** — `mde-vdi-rdp` (ironrdp→egui texture + input) + `mde-vdi-core` (session model) + the `mackesd` session-broker.
4. **Remote desktop over the mesh** — discover (registry+DATACENTER) → connect over Nebula → render → interact. **(Lock 49: the first end-to-end milestone.)** Add `mde-vdi-vnc` fallback.
5. **Local libvirt/QEMU-KVM** — Nova/libvirt lifecycle + OVN provider networking + the VDI broker overlay; fold into Cloud/Instances.
6. **Advanced (fan out):** GPU passthrough · USB redirection · per-monitor VM · live migration · adaptive codec · clipboard/audio/mesh-share bridges.
7. **Port the panels** — Files/Music/Voice as in-shell modules over their existing non-GUI logic.
8. **Packaging** — the immutable **bootc** Workstation image; mesh-only set for headless roles; gh-pages channel.
9. **Decommission** — remove libcosmic/iced + `mde-theme`/`mde-card`; strike the abandoned iced GUI + the retired cosmic-comp tasks; revise `AI_GOVERNANCE.md` §4/§5/§6/§8 + the About/version to 12.0 "Construct".

## Open items — resolved

- All 50 survey questions are locked (above). `E12-0` (governance lock) and `E12-1`
  (the `mde-egui` harness) have landed; this doc **re-scopes E12-2…E12-12** into the
  thin-client VDI execution backlog in `docs/WORKLIST.md` (`## E12` section).
