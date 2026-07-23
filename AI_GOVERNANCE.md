# MCNF — Mackes Cosmic Nebula Fedora — Governance & Architectural Locks

> **Identity:** *MCNF (Mackes Cosmic Nebula Fedora) is a secure, no-fixed-center
> workgroup mesh AND an egui-native mesh **thin-client desktop OS**: the host is a
> single egui shell that owns the DRM seat and brokers VM desktops (local KVM +
> remote mesh peers), with **no Wayland compositor**.* This document is the
> platform's identity + the architectural locks. When a lock conflicts with prose
> elsewhere, the **newer lock wins**; authority ranks **Memory > this file > design
> docs > worklist body**.
>
> **Series (E12 pivot, 2026-06-30; design revised same day; rebranded 2026-07-18):**
> the **12.0 series is codenamed "Construct"**, and the visible product name in
> About, splash, Browser chrome, packaging entries, and other user-facing surfaces
> is **Construct**. The
> package + infra id + GitHub repo **stay `magic-mesh`** so the live-node upgrade
> path is unbroken. Internal identifiers (`mackesd`, `mde-*`, `org.magicmesh.*`,
> `magic-mesh.repo`, `magic-mesh-v*` tags, the `magic-mesh` icon name) are
> unchanged. **E12 first proposed forking the COSMIC compositor; that fork is
> RETIRED — the "C" in MCNF is heritage (the Cosmic-derived look + the `cosmic-*`
> lineage), not a vendored compositor.** Desktop design:
> `docs/design/quasar-vdi-desktop.md`.

This rewrites the 11.x/Cosmic-era governance for the **E12 pivot** (the 12.0
series, "Construct"). E11 ended the labwc desktop and made MCNF a *tenant* of upstream
Cosmic; E12 makes the desktop a first-class, mesh-native part of the platform —
every surface **egui**. The **revised** E12 (50-Q survey, 2026-06-30) is a
**thin-client VDI** desktop: a single egui shell **owns the DRM seat directly (no
compositor)** and brokers VM desktops; the originally-proposed `cosmic-comp` fork
is **retired**. The toolkit (§4), desktop model (§5), and boundary (§6) locks are
restated for that model; the trust envelope (§8) is raised for VM guests. The mesh
substrate (§1), Bus (§2), crypto (§3), Definition of Done (§7), planes (§9), and
build environment (§10) carry forward.

> **Heritage — the Cosmic-era desktop locks (E11) are archived.** "libcosmic's
> vendored iced fork", strictly-IBM-Carbon, "Cosmic provides the desktop / MCNF
> integrates into it", and the mesh/desktop boundary as a two-bucket gate are
> **retired**. Their text lives in git history (pre-2026-06-30 `AI_GOVERNANCE.md`)
> and `docs/design/cosmic-magic-mesh-egui.md`.

## §0 — Master rule

**"Secure, Simple, No-Fixed-Center Workgroup."** Every decision serves a mesh of
peers with no mandatory central authority: any node can author fleet state, any
node can leave, and the fabric heals. *(Unchanged from E11 — the load-bearing
master rule.)*

## §1 — The mesh substrate (load-bearing)

*(Carried forward unchanged from E11.)*

- **Nebula encrypted overlay** is the transport. No Tailscale/Headscale/DERP — the
  fabric is Nebula-native (a lint gate enforces this).
- **No fixed center.** Fleet desired-state is a **versioned revision** any node's
  Workbench can author; revisions gossip peer-to-peer (hop-relay via a lighthouse)
  and every node elects the newest deterministically. `mackesd` reconciles locally.
  (`magic-fleet` is the node engine; the Automation Mesh wraps Ansible per-node,
  Podman-isolated.)
- **Substrate split (SUBSTRATE-V2):** mesh **coordination** (leader election, the
  peer directory, health) lives in **etcd** over Nebula; **files** sync via
  **Syncthing** full-mesh on `/mnt/mesh-storage`. LizardFS and kin are
  retired/forbidden. `~/Local/` is never mesh-synced.
- **`mackesd`** is the supervised control-plane daemon — owns the worker pool, the
  mesh/CA state, the KDC host, and the SQLite store. One leader per the etcd
  lease/election. **E12 adds:** `mackesd` also supervises the **desktop session**
  (start/restart/health) and runs the **desktop-state worker** (§5/§9).
- **Public boundary — 3 tiers (CONNECT):** Public (Nebula/4242 + SSH/22 +
  enroll/4243) · Mesh · Ingress-exposed (lighthouse reverse-proxy). Posture is
  **mesh-allow / public-deny**, drift-corrected firewalld on every node.
- **CONSTRUCT-CLOUD cutover (newest lock 2026-07-22):** cloud desired state and
  OpenTofu state remain mesh-coordinated through etcd; Ansible inventory derives
  from the live mesh roster. Nebula remains the substrate. Libvirt networks use
  NetworkManager/nmstate and never replace the overlay. The retired OpenStack
  coordination and name-service stack is not part of production architecture.

## §2 — The Bus (not D-Bus)

*(Carried forward unchanged from E11.)* Surfaces and `mackesd` communicate over
**`mde-bus`** (internal pub/sub). MDE-private D-Bus is retired; the only
`dbus`/`zbus` use is FDO interop. A lint gate forbids new MDE-private bus names
(`lint-bus-names.sh`, in CI). Bus calls degrade gracefully with no mesh / no
peers: cached state, Bus timeouts, never panic. **E12 adds:** desktop-state
topics (per-peer workspaces, session/overlay state) ride the Bus.

## §3 — Security: maximum crypto

*(Carried forward unchanged from E11.)* Pinned to the strongest interoperable
values, asserted by config tests: **Ed25519** node identity · **AES-256-GCM** /
**ChaCha20-Poly1305** · **XChaCha20-Poly1305** CA backup · **RSA-4096** KDC device
identity. No OpenSSL — **rustls** throughout. The loopback debug-SSH
(NET-INTROSPECT) and documented MD5 interop exceptions (thumbnail cache, Subsonic
auth, SIP digest) stand as recorded. The no-OpenSSL/rustls lock governs MCNF's
own code; independently hosted workloads may bring their own crypto stacks.
Typed platform control traffic remains authenticated and confined to the mesh;
workload-provider APIs are never exposed as a second public control plane.

## §4 — Look & toolkit: egui-native, Apple-HIG-principled (2026-07-22 — replaces the Win10-hybrid chrome direction)

- **The UI toolkit is egui.** Every MCNF surface — shell chrome, panel, session
  view, HUD/overlays — is rendered with **egui** via **eframe** (egui + wgpu) on the
  shared **`mde-egui`** harness, which **owns the DRM/KMS seat directly** (a
  winit-less smithay DRM/GBM + libinput runner — **no Wayland compositor**, §5).
  There is one rendering idiom and one shell across the whole platform. **libcosmic
  / the vendored iced fork is retired.**
- **The design standard is Apple's Human Interface Guidelines, applied as
  principles** (operator lock 2026-07-22, 50-Q survey;
  <https://developer.apple.com/design/human-interface-guidelines>). The platform
  has exactly **two interfaces — Construct (workstation) and Car** — whose
  requirements live in the single authority doc
  **`docs/design/platform-interfaces.md`**. Structure is iPadOS-derived with
  macOS pointer manners; identity stays Quazar-dark; the **Mackes-Carbon icon
  set is kept platform-wide**. All other interface-paradigm design docs are
  retired to `docs/design-archive/`. Do not re-introduce a bottom taskbar, tray
  flyouts, dock, or Start-style panel.
- **The design system is egui-native.** Strict IBM Carbon is **retired**. The
  single source of look is the shared **`Style`/`Visuals` module** in `mde-egui`
  — a Rust module, not a token crate. Surfaces never hand-roll styling; they use
  the shared `Style`. *(Deliberate §0-Simple lever: there is **no** raw-literal /
  Carbon-token lint gate — the shared `Style` module is the discipline.)*
- **Browser workspace exception (operator 2026-07-15).** Browser chrome,
  Bookmarks, engine controls, tabs, toolbar, menus, and Browser page/action
  surfaces are **not** constrained by Carbon wording and should apply Material
  Design 3 principles locally: adaptive layouts, clear top-app-bar/tab hierarchy,
  tonal surfaces/elevation, explicit interaction states, accessible focus/contrast,
  and purposeful motion. This is a Browser-only design direction, not permission
  to migrate unrelated shell/workspace surfaces away from `mde-egui::Style`.
- **Motion** is governed by the operator-locked **MOTION-DRM** epic
  (2026-07-15). The old `animate_bool`-only guidance is superseded: `mde-egui`
  may own a small centralized, DRM-aware motion subsystem with shared durations,
  easing, spring presets, animated values, lifecycle phases, reduced-motion
  handling, and repaint scheduling for the production egui_glow/EGL/GBM/DRM path.
  Compositor-provided animation, Wayland/Xorg dependencies, a renderer rewrite,
  and scattered per-widget timing literals remain out of bounds.
- **Accessibility is deferred** for the 12.0 cutover (egui/eframe carries an
  accesskit path to wire later). A11y is a post-cutover epic, not a Definition-of-
  Done gate for E12 surfaces.
- **Retired §4 lint gates:** `lint-carbon-tokens.sh`, `lint-motion.sh`,
  `lint-libcosmic-rev.sh`, `lint-no-cratesio-iced.sh` are removed from CI.

## §5 — Desktop model & roles (E12 revised — egui-DRM thin-client VDI, no compositor)

> **Revised 2026-06-30** (50-Q `/plan` survey → `docs/design/quasar-vdi-desktop.md`).
> The forked-`cosmic-comp` desktop is **retired** before any code landed; MCNF does
> **not** fork or ship a Wayland compositor.

> **NEWEST LOCK 2026-07-22 — zero-OpenStack local-first cloud cutover.** The
> transitional OpenStack/Kolla/Nova/Heat/Keystone architecture is retired and
> its production code has been deleted. Do not reintroduce it as a backend or
> compatibility shim. Construct Cloud exposes provider-neutral Workloads
> contracts through typed `mackesd` Bus verbs. **OpenTofu provisions** local
> **libvirt/QEMU-KVM** VMs and networks; **Ansible configures** nodes and
> workloads from mesh-derived inventory; **Podman + Quadlet** delivers service
> containers; bootc + osbuild/image-builder owns images. Tofu state is
> etcd-backed, secrets come only through mde-seal, and Nebula remains the mesh
> network. `cloud-hypervisor` and the OpenStack control plane are retired.

- **The host is an egui thin client, not a general desktop.** The whole UI is a
  single **egui shell that owns the DRM/KMS seat directly** (the §4 `mde-egui` smithay
  DRM/GBM + libinput runner) — **no Wayland compositor**. There are **no native host
  apps**: a browser / office suite / game runs **inside a VM guest**, never on the
  host. A thin persistent chrome bar (peers · sessions · status) frames the active
  desktop and **expands into the full Workbench**.
- **The desktop you use is a VM.** A Workstation **brokers and displays full OS
  desktops** (Windows/Ubuntu/…) that run either **locally on libvirt/QEMU-KVM
  through the typed Workloads plane** or **remotely on any mesh peer** (a
  **headless Workstation** serving VM desktops over the same libvirt/QEMU-KVM stack), rendered
  **egui-native** (ironrdp/vnc → an egui texture) over Nebula. A "session" is a
  fullscreen VM desktop; sessions **roam** per-peer via etcd/Syncthing. The
  mesh-control surfaces (Workbench/Files/Music/Voice) are **panels inside the one
  shell**, not separate clients.
- **One stack, two roles (revised 2026-06-30 — `docs/design/onboarding-wizard.md` +
  `mesh-virt-management.md`).** **Lighthouse · Workstation** (rank 0/1). Every machine
  runs the **byte-identical stack**; **role is configuration, not a build** — a flag
  toggles systemd units, so a box is re-roled without a reinstall. A **headless machine
  is a Workstation without a local display** (daemon stack only, no egui seat, serving
  VMs/containers to the mesh). The **Lighthouse** is relay + control plane + **media
  server (Navidrome→DO Spaces) + CA/signer**. There is **no XCP-NG role** — the
  hypervisor is **Fedora + libvirt/QEMU-KVM + OVN + Podman**; the old
  cloud-hypervisor path is deleted/retired, not the target stack. An external
  XCP-ng host may be *adopted* day-2 but is never
  produced by our installer. A **`desktop-host`** tag marks peers serving VM
  desktops; `mackesd` runs the **session-broker + vm-lifecycle + container**
  workers (the shell renders; mackesd brokers — §1/§9).
- **Delivery:** **one immutable bootc/ostree image for every role** (egui-DRM shell +
  libvirt/QEMU-KVM + OVN + ironrdp/VNC + `mackesd` + Podman + Nebula baked in,
  plus the **seat services — PipeWire/WirePlumber · BlueZ · UPower** — per the
  host-controls locks, `docs/design/quasar-host-controls.md` / E12-15..19, and
  **UDisks2 + the fs toolchain**
  (e2fsprogs/xfsprogs/btrfs-progs/exfatprogs/ntfs-3g/cryptsetup/qemu-img) per the
  Storage-plane locks, `docs/design/workbench-storage-plane.md` / E12-20..23; VM
  disks + mesh state on the writable partition). The **role is a config flag**,
  not a separate build — a Lighthouse runs the same image with the desktop units
  masked (Option 1), role-features migrating to managed Podman/VM workloads over
  time (Option 2). The install-time **role chooser** (binary: Lighthouse /
  Workstation) + the GitHub-hosted dnf repo (Releases asset + GitHub Pages,
  project-GPG-signed) carry forward.

## §6 — The boundary: layered tiers (E12 — replaces the two-bucket gate)

The dependency graph is **three lint-gated tiers**:

```
  desktop-shell   ⊃→   platform-services   ⊃→   mesh-substrate
  (the one egui       (mackesd: session-        (Nebula, etcd,
   shell on DRM +       broker + vm-lifecycle,    Syncthing, CA/KDC)
   mde-vdi)             mde-bus, magic-fleet)
```

- **Dependencies point only inward** (shell → services → mesh; never outward). A
  dependency edge that points outward (substrate → services, or services → shell)
  is a **CI failure**, enforced by the layered-tiers gate that replaces
  `lint-mesh-boundary.sh`. This keeps the mesh **headless-capable** — the
  substrate and services never pull a desktop dependency.
- New code is **glue, not reimplementation** — reuse existing crates.
- **Public network boundary (CONNECT) is unchanged:** the internet-facing surface
  is default-deny; MCNF only ever *widens* it for an explicitly-exposed service;
  intra-mesh trust stays flat / open-mesh (§8).

> **Notification interface (NOTIFY-CHAT, operator survey 2026-07-02 — `docs/design/mesh-chat-icq.md`).**
> The one notification surface is an **ICQ-style mesh chat**: every host (local + remote, incl. VM
> guests) is a roster **contact with its hostname as the username**, and its system alerts + clipboard
> copies arrive as **signed messages from that contact** (a mackesd `chat` worker subscribes every alert
> lane and folds them, no emitter changes). Group + 1:1 chat over **Bus-live + Syncthing-history**
> (Ed25519-signed, Nebula transit). This **subsumes and removes** the standalone Notifications AND
> Clipboard surfaces. Successor to the retired-iced NOTIFY-REDESIGN.

## §7 — Definition of Done

*(Carried forward unchanged from E11 — still load-bearing.)* A change is done only
when it is **runtime-reachable and observably works** — no `todo!()`/
`unimplemented!()`, no stub match arms, no `pub mod` with zero external refs, no
mockups/`demo_data` passing as features. Builds clean, tests green, clippy + fmt
clean. *(The E11 visual-confirmation gate was already lifted; under E12 the
look-source is the shared egui `Style` module rather than Carbon tokens, but the
runtime-reachability bar is unchanged — `/preview` stays optional/best-effort.)*

## §8 — Positioning & trust envelope

**Production workgroup-grade, not hyperscale.** Infrastructure envelope: a single
workgroup of up to **3 lighthouses + 9 peers** of the Lighthouse/Workstation
roles. Trust stays **flat / open-mesh** (any enrolled cert reaches every peer +
service; no per-service ACL) — the §0 "Simple" lever, accepted because the envelope
is a small trusted workgroup; the blast radius is documented for operators. Security
controls are **enforced in code + covered by tests** (single-use enrollment bearer;
revocation evicts the data plane; unpinned node fails closed; hash-chain audit).

> **VDI revision (2026-06-30 — `quasar-vdi-desktop.md` locks 46/47).** **VM desktop
> guests are first-class mesh members** — each dual-homed (its own Nebula cert + a
> LAN NIC) and a **full flat-trust peer**. The supported node envelope is **raised**
> to accommodate them; guest count is no longer bounded by the 12-node
> infrastructure cap. This **widens the flat-trust blast radius** (a guest OS — incl.
> Windows — reaches every peer + service); that radius **MUST be documented for
> operators** (extends ENT-12), guests stay **default-deny inbound**, and
> per-service ACLs are revisited if the envelope grows materially.

> **CONSTRUCT-CLOUD revision (newest lock 2026-07-22).** Local libvirt VMs and
> Podman workloads are not silently treated as enrolled mesh peers. Network and
> exposure are declared through the provider-neutral Workloads contract;
> Nebula enrollment remains the boundary for full flat-mesh trust. The compute
> envelope may grow beyond the infrastructure-peer cap, but doing so does not
> create a second identity, quota, or authorization authority.

## §9 — The five planes

*(Carried forward from E11, restated for egui.)* The Workbench's mesh IA is **five
planes**: **This Node** · **Controller** · **Network** · **Fleet** ·
**Provisioning** — with the **Peers directory as the Front Door** and
desktop-personal panels grouped below. Locks: **no RBAC** (access to the mesh IS
the control plane) · **2 roles + capability tags** (hop/execution/headless) ·
**the Controller is a plane, not a place** (etcd + Syncthing; the elected leader
only coordinates) · **remote execution is typed verbs + signed job bundles only**
(no raw shell) · **one state doctrine** (etcd + TOML/YAML on Syncthing + typed
`mackesd` Bus verbs; GUIs are renderers; CLI parity) · **mesh tooling first**
(D-W1). **E12 note:** the Workbench is now an **egui** client; the plane IA and
the renderers-not-authorities doctrine are unchanged. **E12 adds** the desktop
plane's per-peer workspace + mesh-overlay state to the one-state doctrine
(etcd/Syncthing-backed).

> **CONSTRUCT-CLOUD amendment (newest lock 2026-07-22).** The Controller plane's
> workload functions are exposed as provider-neutral Construct Cloud contracts:
> workloads, images, networks, containers, configuration, lifecycle, and status.
> GUIs remain renderers; typed `mackesd` verbs own validation, placement,
> authorization, and audit. The current adapter renders OpenTofu for provisioning
> and runs Ansible for configuration against local libvirt/KVM, NetworkManager,
> Podman/Quadlet, and bootc/osbuild. `mde-bus` remains the only platform bus;
> etcd plus declared files remain the one-state authority. No provider-specific
> identity or control plane may become a second platform authority.

## §10 — Build & development environment (canonical — do not rediscover)

*(Carried forward unchanged from E11.)* **§10.0 MANDATE: all AIs work the farm,
scale out, never grind solo** — every AI/agent operating this repo must use the
build farm for build/test/gate work whenever the task is not a tiny local
syntax/probe check. **This is a standing directive for all AI surfaces and
subagents: prefer farm execution, parallelize independent gates, and document any
local-only exception in the handoff.** Heavy or decomposable work is offloaded to
the build farm and run in parallel across farm nodes by default; a slow/fuzzy
success signal is never a reason to serialize or defer (see `no-flinch`). Keep
independent checks on separate hosts/slots, avoid containerized fixtures when a
direct farm-host fixture works, and treat farm/test hosts as safe for destructive
reboot/recovery operations unless a worklist item explicitly says otherwise.
Mechanics (build slots, per-node concurrency caps, worktree-isolated parallel
mutating agents) and the full toolchain live in
[`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md) — **read it before
building.** Load-bearing facts: two build surfaces (local dev
host + the Fedora farm VMs); Rust pin `1.94.0` / MSRV `1.85`; the
`opus-devel`-in-CRB EL9 trap; the farm is IaC (`infra/tofu/` + `infra/ansible/`);
the **GitOps reconciler on a timer** is the canonical build lane (no AI in the
build loop). **E12 note:** the GUI build is now an **egui/eframe** compile over
the direct DRM/GBM runner; update the farm's GUI build expectations accordingly
(libcosmic and the forked compositor are gone).

**§10.0.1 — BigBoy takes the longest / most-complex build (standing rule, operator
2026-06-30).** The single heaviest job always routes to **XEN-BIGBOY**
(`172.20.0.130`, 12 vCPU / ~20 GiB — the high-capacity build VM): a full
`cargo --workspace` build/test/clippy, the biggest egui crates
(`mde-shell-egui`), a cold egui/eframe/wgpu compile, or the RPM
release build. The 4-vCPU nodes (`.50` / `.90` / `.170`) take the shorter/simpler
jobs (single small crates, per-crate tests/clippy). This composes with the ≤-cap
spread (`docs/BUILD-ENVIRONMENT.md`): spread the *count* to honor per-node caps,
but the *long pole* goes to BigBoy first — never leave the workspace/heavy-GUI
build on a small node while BigBoy runs a trivial one. The **canonical farm
roster** is `install-helpers/farm-topology.sh` — **4 dom0s / 4 build VMs / 9 heavy
slots** (`.50`/`.90`/`.170` at cap 2 · BigBoy `.130` at cap 3; the 4th dom0 is
XEN-194 → `mcnf-build-53` → `.170`). Run `./install-helpers/farm-topology.sh table`
at the start of every run for a **verified** utilization table — it probes all 4
nodes and **fails if one is missing** (XEN-194/.170 sat idle a whole session once,
under a stale 3-node roster). Never hardcode the node list or chart from memory.
Stale `10.0.0.x` farm pins are invalid; the build VMs live on the `172.20.0.x`
lanes above. Browser/CEF live probes are not generic farm assumptions: use the
packaged `/opt/mde/cef` runtime only where installation has staged it, or the
known warm `.50` bundle at `$HOME/mde-cef-active`. Cold Servo/browser test
builds can exhaust small-node slot disks through Rust incremental caches; put
those long-pole jobs on BigBoy and use `CARGO_INCREMENTAL=0` after any ENOSPC
failure.

---

*Heritage: the pre-E12 Cosmic-era identity (libcosmic/iced, strictly-Carbon,
Cosmic-as-tenant) lives in the git history of `AI_GOVERNANCE.md` and in
`docs/design/cosmic-magic-mesh-egui.md`. The pre-pivot labwc-era identity lives in
the archived [MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation)
repo, whose labwc-era desktop the E11 pivot end-of-lifed.*
