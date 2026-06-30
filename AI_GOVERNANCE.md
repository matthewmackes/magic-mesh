# MCNF — Mackes Cosmic Nebula Fedora — Governance & Architectural Locks

> **Identity:** *MCNF (Mackes Cosmic Nebula Fedora) is a secure, no-fixed-center
> workgroup mesh AND its own forked, mesh-native Fedora-Cosmic desktop OS, with
> an all-egui UI.* This document is the platform's identity + the architectural
> locks. When a lock conflicts with prose elsewhere, the **newer lock wins**;
> authority ranks **Memory > this file > design docs > worklist body**.
>
> **Series (E12 pivot, 2026-06-30):** the **12.0 series is codenamed "Quasar"**,
> surfaced as `MCNF 12.0 "Quasar"` in the About/greeter. The package + infra id +
> GitHub repo **stay `magic-mesh`** so the live-node upgrade path is unbroken.
> Internal identifiers (`mackesd`, `mde-*`, `org.magicmesh.*`, `magic-mesh.repo`,
> `magic-mesh-v*` tags, the `magic-mesh` icon name) are unchanged. The **"C" in
> MCNF now means *our forked Cosmic desktop*, not "runs on stock Cosmic"** — E12
> forks the COSMIC source into the repo.

This rewrites the 11.x/Cosmic-era governance for the **E12 pivot** (the 12.0
series, "Quasar"). Where E11 ended the labwc desktop and made MCNF a *tenant* of
upstream Cosmic, **E12 forks Cosmic into the repo** and makes the desktop a
first-class, mesh-aware part of the platform — every surface an **egui** Wayland
client. The toolkit (§4), desktop-ownership (§5), and boundary (§6) locks are
restated for the forked-egui context. The mesh substrate (§1), Bus (§2), crypto
(§3), Definition of Done (§7), trust envelope (§8), planes (§9), and build
environment (§10) carry forward.

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
auth, SIP digest) stand as recorded.

## §4 — Look & toolkit: egui-native (E12 — replaces strictly-Carbon)

- **The UI toolkit is egui.** Every MCNF surface — shell chrome, panel, apps,
  HUD/overlays — is rendered with **egui** via **eframe** (egui + winit + wgpu),
  built as a **Wayland client** on the shared **`mde-egui`** harness. There is one
  rendering idiom across the whole platform. **libcosmic / the vendored iced fork
  is retired.**
- **The design system is egui-native.** Strict IBM Carbon is **retired**. The
  single source of look is the shared **`Style`/`Visuals` module** in `mde-egui`
  — a Rust module, not a token crate. Surfaces never hand-roll styling; they use
  the shared `Style`. *(Deliberate §0-Simple lever: there is **no** raw-literal /
  Carbon-token lint gate — the shared `Style` module is the discipline.)*
- **Motion** uses egui's built-in animation (`animate_bool` / `ctx` animation)
  driven by a small shared duration+easing table in `mde-egui`. There is no
  bespoke motion module and no motion lint gate.
- **Accessibility is deferred** for the 12.0 cutover (egui/eframe carries an
  accesskit path to wire later). A11y is a post-cutover epic, not a Definition-of-
  Done gate for E12 surfaces.
- **Retired §4 lint gates:** `lint-carbon-tokens.sh`, `lint-motion.sh`,
  `lint-libcosmic-rev.sh`, `lint-no-cratesio-iced.sh` are removed from CI.

## §5 — Distribution, desktop ownership & roles (E12 — replaces "Cosmic provides the desktop")

- **MCNF forks and owns the desktop.** The COSMIC source — `cosmic-comp` (smithay
  compositor) + `cosmic-panel` + `cosmic-session` + `cosmic-settings` — is
  **vendored into the repo**, pinned to a recorded upstream rev and rebased on a
  cadence. The forked `cosmic-comp` stays a **pure compositor** (no UI embedded);
  all UI is egui Wayland clients on top.
- **The compositor is mesh-aware (this is what earns the fork).** The forked
  session provides **per-peer workspaces**, a **mesh overlay/HUD**, and
  **desktop-state** surfaced from etcd/Syncthing; `mackesd` supervises the
  session. A pure, non-mesh fork would not justify the maintenance burden.
- **Two delivery lanes from one codebase, because the surfaces are portable
  Wayland clients:** (1) the **integrated spin** — forked `cosmic-comp` + the egui
  shell/panel/app clients, built by a Fedora-Cosmic **kickstart**; (2) the **RPM
  layer** — the same egui client binaries installed onto **stock** upstream
  Fedora-Cosmic. Headless **Server/Lighthouse** roles install the **mesh-only RPM
  set** (no desktop).
- **Deployment roles** (Lighthouse ⊂ Server ⊂ Workstation, strict supersets) gate
  which `mackesd` workers + surfaces run. "Workstation" = the MCNF desktop (spin
  or RPM-layer). Shipped as **one RPM + install-time role chooser** + a
  GitHub-hosted dnf repo (Releases asset + GitHub Pages, project-GPG-signed) + the
  kickstart spin.

## §6 — The boundary: layered tiers (E12 — replaces the two-bucket gate)

The dependency graph is **three lint-gated tiers**:

```
  desktop-shell   ⊃→   platform-services   ⊃→   mesh-substrate
  (egui clients +      (mackesd, mde-bus,        (Nebula, etcd,
   forked COSMIC)       magic-fleet, enroll,      Syncthing, CA/KDC)
                        session-supervisor)
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

## §7 — Definition of Done

*(Carried forward unchanged from E11 — still load-bearing.)* A change is done only
when it is **runtime-reachable and observably works** — no `todo!()`/
`unimplemented!()`, no stub match arms, no `pub mod` with zero external refs, no
mockups/`demo_data` passing as features. Builds clean, tests green, clippy + fmt
clean. *(The E11 visual-confirmation gate was already lifted; under E12 the
look-source is the shared egui `Style` module rather than Carbon tokens, but the
runtime-reachability bar is unchanged — `/preview` stays optional/best-effort.)*

## §8 — Positioning & trust envelope

*(Carried forward unchanged from E11.)* **Production workgroup-grade, not
hyperscale.** Supported envelope: a single workgroup of up to **3 lighthouses + 9
peers (12 nodes)**. Trust stays **flat / open-mesh** (any enrolled cert reaches
every peer + service; no per-service ACL) — the §0 "Simple" lever, accepted only
because the envelope is a small trusted workgroup; the blast radius is documented
for operators. Security controls are **enforced in code + covered by tests**, not
just documented (single-use enrollment bearer; revocation evicts the data plane;
unpinned node fails closed; hash-chain audit).

## §9 — The five planes

*(Carried forward from E11, restated for egui.)* The Workbench's mesh IA is **five
planes**: **This Node** · **Controller** · **Network** · **Fleet** ·
**Provisioning** — with the **Peers directory as the Front Door** and
desktop-personal panels grouped below. Locks: **no RBAC** (access to the mesh IS
the control plane) · **3 roles + capability tags** (hop/execution/headless) ·
**the Controller is a plane, not a place** (etcd + Syncthing; the elected leader
only coordinates) · **remote execution is typed verbs + signed job bundles only**
(no raw shell) · **one state doctrine** (etcd + TOML/YAML on Syncthing + typed
`mackesd` Bus verbs; GUIs are renderers; CLI parity) · **mesh tooling first**
(D-W1). **E12 note:** the Workbench is now an **egui** client; the plane IA and
the renderers-not-authorities doctrine are unchanged. **E12 adds** the desktop
plane's per-peer workspace + mesh-overlay state to the one-state doctrine
(etcd/Syncthing-backed).

## §10 — Build & development environment (canonical — do not rediscover)

*(Carried forward unchanged from E11.)* **§10.0 MANDATE: work the farm, scale out,
never grind solo** — heavy/decomposable work is offloaded to the build farm and
run in parallel across the farm nodes; a slow/fuzzy success signal is never a
reason to serialize or defer (see `no-flinch`). Mechanics (build slots, per-node
concurrency caps, worktree-isolated parallel mutating agents) and the full
toolchain live in [`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md) —
**read it before building.** The live farm topology (hosts + build VMs + free
slots) is **derived from OpenTofu, never memorized**: run
`install-helpers/farm-inventory.sh topology` (the single source every consumer
reads) instead of trusting a host list from a prior context. Load-bearing facts:
two build surfaces (local dev host + the **4-dom0** Fedora farm VMs); Rust pin `1.94.0` / MSRV `1.85`; the
`opus-devel`-in-CRB EL9 trap; the farm is IaC (`infra/tofu/` + `infra/ansible/`);
the **GitOps reconciler on a timer** is the canonical build lane (no AI in the
build loop). **E12 note:** the GUI build is now an **egui/eframe** compile
(winit + wgpu) plus the forked-compositor crates; update the farm's GUI build
expectations accordingly (libcosmic is gone).

---

*Heritage: the pre-E12 Cosmic-era identity (libcosmic/iced, strictly-Carbon,
Cosmic-as-tenant) lives in the git history of `AI_GOVERNANCE.md` and in
`docs/design/cosmic-magic-mesh-egui.md`. The pre-pivot labwc-era identity lives in
the archived [MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation)
repo, whose labwc-era desktop the E11 pivot end-of-lifed.*
