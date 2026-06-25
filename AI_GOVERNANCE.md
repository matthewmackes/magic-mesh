# MCNF — Mackes Cosmic Nebula Fedora — Governance & Architectural Locks

> **Identity:** *MCNF (Mackes Cosmic Nebula Fedora) is a secure, no-fixed-center
> workgroup mesh and its native-Rust Carbon GUIs, running on stock Fedora-Cosmic.*
> This document is the platform's identity + the architectural locks. When a lock
> conflicts with prose elsewhere, the **newer lock wins**; authority ranks
> **Memory > this file > design docs > worklist body**.
>
> **Naming (rebrand 2026-06-17):** the product is **MCNF — Mackes Cosmic Nebula
> Fedora**. The **10.0.x series is codenamed "Magic Mesh"**, surfaced as
> `MCNF 10.0 "Magic Mesh"` in the About/greeter. The package + infra id + GitHub
> repo **stay `magic-mesh`** (which *is* the codename) so the live-node upgrade
> path is unbroken; the package may rename to `mcnf` at the **11.0 series
> boundary**, and future series get new codenames. Internal identifiers
> (`mackesd`, `mde-*`, `org.magicmesh.*`, `magic-mesh.repo`, `magic-mesh-v*`
> tags, the `magic-mesh` icon name) are unchanged — only the display/product name
> changed from "Magic Mesh" to "MCNF".

This rewrites the MackesWorkstation governance for the **E11 pivot** (the 10.0
series, codenamed "Magic Mesh"). The labwc/Win-era desktop is end-of-life; Cosmic
owns the desktop. The locks below are what survives the cutover, restated for the
Cosmic context — the shell/compositor/era-theme/desktop-surface locks are
**retired**.

## §0 — Master rule

**"Secure, Simple, No-Fixed-Center Workgroup."** Every decision serves a mesh of
peers with no mandatory central authority: any node can author fleet state, any
node can leave, and the fabric heals.

## §1 — The mesh substrate (load-bearing)

- **Nebula encrypted overlay** is the transport. No Tailscale/Headscale/DERP — the
  fabric is Nebula-native (a lint gate enforces this).
- **No fixed center.** Fleet desired-state is a **versioned revision** any node's
  Workbench can author; revisions gossip peer-to-peer (hop-relay via a lighthouse)
  and every node elects the newest deterministically. `mackesd` reconciles locally;
  there is no controller SSH-ing in. (`magic-fleet` is the node engine; the
  Automation Mesh wraps Ansible per-node, Podman-isolated.)
- **Substrate split (SUBSTRATE-V2):** mesh **coordination** (leader election, the
  peer directory, health) lives in **etcd** over Nebula (strongly consistent, on
  the anchor nodes); **files** sync via **Syncthing** full-mesh on
  `/mnt/mesh-storage` (a plain dir, no FUSE). LizardFS (and MooseFS/Gluster/Ceph)
  are **retired/forbidden** — the single-mount-carries-everything model caused
  "mount down = mesh down". The LizardFS plane was fully removed in the 11.0 series
  (SUBSTRATE-6). `~/Local/` is never mesh-synced. *(Design: docs/design/substrate-v2.md.)*
- **`mackesd`** is the supervised control-plane daemon — owns the worker pool, the
  mesh/CA state, the KDE-Connect host, and the SQLite store. One leader per the
  **etcd lease/election** (SUBSTRATE-V2; the `.mackesd-leader.lock` is retired at
  the 11.0 cutover); owns writes.
- **Public boundary — 3 tiers (CONNECT, lands 11.0+, design: docs/design/connect.md):**
  **Public** (the internet sees only **Nebula/4242 + SSH/22 + enroll/4243** — the
  foundational layer) · **Mesh** (everything reachable over the Nebula overlay) ·
  **Ingress-exposed** (services published to the public **only** through the
  **lighthouse reverse-proxy ingress** — Caddy auto-HTTPS + allowlisted TCP/UDP
  streams, rendered from a per-service exposure policy). The posture is
  **mesh-allow / public-deny**, enforced by a drift-corrected **firewalld** profile
  on every node. The unified control surface is a **mesh-native Connectivity
  Manager** (Workbench + a `mackesd` worker orchestrating firewalld/nmstate/Nebula
  fw/VPN-GW/DDNS/ingress) — **no external NMS/SDN** (D-W1). This governs the
  **public boundary only**; intra-mesh trust stays **flat / open-mesh** (§8
  unchanged — a valid mesh cert still reaches every peer).

## §2 — The Bus (not D-Bus)

- Surfaces and `mackesd` communicate over **`mde-bus`** (internal pub/sub). MDE-
  private D-Bus is retired; the only `dbus`/`zbus` use is **FDO interop**
  (`org.freedesktop.*` — Notifications host, session FDO — and the FDO MPRIS
  standard `org.mpris.*`). A lint gate forbids new MDE-private bus names
  (`install-helpers/lint-bus-names.sh`, in CI).
- Bus calls **degrade gracefully** with no mesh / no peers: cached state, Bus
  timeouts, never panic.

## §3 — Security: maximum crypto

The fabric is pinned to the strongest interoperable values, asserted by config
tests: **Ed25519** node identity · **AES-256-GCM** / **ChaCha20-Poly1305** ·
**XChaCha20-Poly1305** CA backup · **RSA-4096** KDC device identity. Enrollment
uses max key complexity. No OpenSSL — **rustls** throughout.

**Loopback debug-SSH (NET-INTROSPECT, 2026-06-13):** mackesd enables Nebula's
built-in debug SSH server to classify direct-vs-relay tunnel paths
(`nebula_admin`). It is bound to **`127.0.0.1` only** (never the overlay or the
underlay — no firewall-reachable surface), **key-auth** with an Ed25519 keypair
(0600, under `/etc/nebula`), and shells `ssh`/`ssh-keygen`. Absent those it
degrades silently to the honest `path:"overlay"` — never a guess.

**Documented interop exceptions (not violations — sweep-3 I6/I7):** MD5 where an
external spec mandates it and it carries no MDE security: the freedesktop
thumbnail-cache filename (`mde-files/thumbnails.rs` — cache key per the XDG spec),
the Subsonic API auth token (`mde-musicd/airsonic.rs` — the upstream API's
scheme; mitigate by using TLS to the server), and SIP digest authentication
(`mde-voice-hud/sip.rs` — RFC 3261 mandates MD5 for the digest; it is
server-chosen, so prefer SHA-256 when the registrar offers it). Anything else
MD5/SHA1 is a finding.

## §4 — Look: strictly IBM Carbon

- The GUI is **IBM Carbon** (carbondesignsystem.com) — the only switchable themes
  are Carbon's gray themes: **Gray 10** (light) / **Gray 90** / **Gray 100**
  (default dark). Carbon tokens (type scale, 8px spacing, components, 2px focus,
  motion) are **single-sourced in `mde-theme`** and guarded by a lint gate
  (`install-helpers/lint-carbon-tokens.sh`, in CI). No raw hex / scattered metric
  literals outside the token modules (mark a genuinely-dynamic/test colour
  `// carbon-ok` with a reason).
- **Motion** is part of that single source: every animation's timing comes from
  `mde_theme::motion` (the Carbon duration/easing grid + `Motion` presets + the
  `list`/`icon` stagger tokens) and routes reduce-motion through
  `Motion::resolved`/`Tween::resolved` — never a bespoke `Duration::from_millis`
  literal. A second §4 gate enforces it (`install-helpers/lint-motion.sh`, in CI;
  waive a design-blessed off-grid one-shot with `// motion-ok`). The copy-paste
  patterns (hover, list stagger, modal fade, the `Animator`, the reduce-motion
  fallback) live in **[`docs/design/motion-guide.md`](docs/design/motion-guide.md)**
  — read it before adding motion.
- **Pure-Rust toolkit:** libcosmic's vendored iced fork (wgpu + cosmic-text, no
  FreeType; carries a11y/accesskit — the full-libcosmic cutover, 2026-06-13), rustls (no
  OpenSSL). GUIs ship as Cosmic apps + a native cosmic-applet; Cosmic draws the
  panel/decorations, MCNF draws its client areas.

## §5 — Distribution & roles

- **Cosmic provides the desktop.** MCNF integrates *into* it: a cosmic-applet
  (Bus→Cosmic bridge), notifications via Cosmic, Workbench as the control surface,
  `mde-files` as the default file manager.
- **Deployment roles** (Lighthouse ⊂ Server ⊂ Workstation, each a strict superset)
  gate which `mackesd` workers + surfaces run. "Workstation" = a Cosmic desktop.
- Shipped as one RPM + install-time role chooser, a GitHub-hosted dnf repo (GitHub Releases asset + GitHub Pages, project-GPG-signed), and a
  Magic-on-Cosmic ISO.

## §6 — The boundary

No mesh-side crate may depend on a deleted desktop-shell crate. The split is **lint-
gated** (`lint-mesh-boundary.sh`): the dependency graph must stay clean of the
EOL'd desktop. New code is **glue, not reimplementation** — reuse the existing
crates.

**Public network boundary (CONNECT, 11.0+).** The internet-facing surface is
**default-deny**: firewalld's `public` zone drops everything not explicitly
allowed, and MCNF code only ever **widens** that surface for a service the
operator **explicitly exposed** (the `connect_firewall` worker is *additive* —
it opens a policy-declared ingress port, never a blanket allow, and never removes
the foundational SSH/Nebula/enroll rules). The only always-public layer is the
foundational one (§1 tier-1: Nebula/4242 + SSH/22 + enroll/4243); everything else
reaches the public **only** through the lighthouse reverse-proxy ingress per the
per-service exposure policy. This governs the public boundary only — intra-mesh
trust stays flat / open-mesh (§8).

## §7 — Definition of Done

A change is done only when it is **runtime-reachable and observably works** — no
`todo!()`/`unimplemented!()`, no stub match arms, no `pub mod` with zero external
refs, no mockups/`demo_data` passing as features. Builds clean, tests green, clippy
+ fmt clean.

**Visual-confirmation gate lifted (2026-06-11, operator directive).** Render
correctness now rests on the `mde-theme` Carbon tokens (§4 — still enforced: no raw
hex / scattered metric literals) plus palette/component tests and render-agnostic
logic. Operator or on-session (`/preview`) visual confirmation is **no longer a
condition of Done** — a GUI change that builds, tests green, and renders through the
Carbon tokens is §7-complete. A `/preview` check remains available but is
optional/best-effort, never a blocker (it stays hardware-gated and so cannot hold a
feature on a headless host).

## §8 — Positioning & trust envelope (2026-06-09, enterprise-readiness verification)

- **Production workgroup-grade, not hyperscale.** The supported envelope is a **single workgroup of
  up to 3 lighthouses + 9 Headless/Full peers (12 nodes)** — operator directive 2026-06-14, raising
  the prior ≤ 8-peer / single-lighthouse cap; `ca/sign.rs` `MAX_PEER_CAP` follows. Up to 3 public
  lighthouses give NAT relay/discovery + etcd-quorum + Mesh-Sync (Syncthing) redundancy (LH1 = founding CA holder;
  LH2/LH3 = additional lighthouses, one CA on LH1 — see `docs/design/magic-setup-wizard.md`). Within
  this envelope the bar is **reliable + operable + documented**. Going beyond 3 LH + 9 peers /
  full multi-CA HA / multi-tenant is **out of scope** for this identity.
- **Open-mesh is a deliberate trade-off, honestly documented.** Trust stays **flat** (any enrolled
  cert reaches every peer + every service; no per-service ACL) — the §0 "Simple" lever. This is
  accepted *only* because the envelope is a small trusted workgroup; the blast radius MUST be
  documented for operators (ENT-12). Least-privilege/per-service ACLs are explicitly deferred.
- **Security controls must be enforced, not just documented.** The §3 crypto locks are necessary but
  not sufficient: a control that prose claims (e.g. the enrollment bearer) MUST be enforced in code
  and covered by a test. Specifically: enrollment validates a **single-use issued bearer**;
  revocation **evicts the data plane** (nebula `pki.blocklist` + reload), not just the DB; an
  **unpinned node fails closed** (refuses to start) rather than defaulting to Workstation; security
  events are **hash-chain audited**. (Corrective decisions C1–C3, C10.)

## §9 — The five planes (2026-06-09, PLANES survey W1–W100)

The Workbench's mesh IA is **five planes**: **This Node** (host agent) · **Controller**
(jobs, remediation, audit, policy) · **Network** · **Fleet** (rollups) ·
**Provisioning** — with the **Peers directory as the Front Door** above them and
desktop-personal panels grouped below. Locks (full table: `docs/design/planes.md`):

- **No RBAC — access to the mesh IS the control plane.** A valid mesh cert is the
  authorization; the desktop is the operator (extends §8's flat-trust envelope). (W1)
- **3 roles + capability tags.** §5's roles stay the install-time identity;
  **hop / execution / headless** are orthogonal, **gating** tags — untagged duty is
  refused. (W2/W82/W84)
- **The Controller is a plane, not a place.** No-fixed-center holds: coordination
  state lives in **etcd** + bulk files on **Mesh Sync (Syncthing)**, any node hosts
  the surfaces, the elected leader (an etcd lease) only coordinates (schedules,
  sweeps). (W3)
- **Remote execution is typed verbs + signed job bundles only — no raw shell
  channel, ever.** Jobs are Ansible playbooks the *target* runs locally; no
  push-SSH. (W21/W32)
- **One state doctrine:** every plane's durable state = **etcd** (coordination) +
  TOML/YAML dirs on **Mesh Sync (Syncthing)** (files) + typed `mackesd` Bus verbs;
  GUIs are renderers; every surface has CLI parity. (W88/W27)
- **Mesh tooling first, Red Hat best practices second** (standing directive D-W1):
  FPG/Bus/etcd/Syncthing/Nebula before new components; Ansible/nmstate/firewalld/
  kickstart/createrepo where the mesh has no native tool. Greenfield — no legacy
  carried.
- **Hero images:** sections backed by an external project carry a Carbon-compliant
  line-art hero (original homages, `mde-theme`-compiled, `hero_stroke` token). (H1–H10)

## §10 — Build & development environment (canonical — do not rediscover)

> **§10.0 — MANDATE: work the farm, scale out, never grind solo.** *(operator,
> 2026-06-22, after a full session ran sequentially on one node while the farm sat
> idle.)* Heavy or decomposable work **MUST** be offloaded to the build farm and run
> **in parallel across the farm nodes** (`172.20.0.50/.51/.52`) — not serialized on one
> node, and not done as a single sequential loop when the work splits.
> 1. **Builds/tests/RPM cuts run on the farm** (`install-helpers/xcp-build.sh`), never
>    blocking the orchestration loop on a local compile when the farm is reachable.
> 2. **Decompose and fan out.** When work splits into file-disjoint pieces (a worker
>    handler + its GUI panel; N independent tasks), spawn **concurrent subagents** —
>    each owning disjoint files and building on a **different farm node** — instead of
>    doing them one at a time. Distribute builds across `.50/.51/.52`.
> 3. **A slow or fuzzy success signal is never a reason to defer, serialize, or
>    reclassify work as "tail"** (see the `no-flinch` skill). "Harder/slower for me" ≠
>    "lower priority for the operator." Fix the feedback loop (distribute the build);
>    don't route around the work.
> 4. The measure is **throughput on the operator's priorities**, not the cleanliness of
>    a single sequential green checkmark. Verify you are actually parallelizing.
>
> **Mechanics (learned in practice):**
> - **Concurrent jobs per node:** `MCNF_BUILD_SLOT=<n>` gives `xcp-build.sh` an isolated
>   remote workspace+target on one VM, so several builds share a host without colliding
>   (BigBoy `.52`, 12c/24G, runs 2-3 in parallel). Distribute agents across `.50/.51/.52`
>   AND across slots.
> - **Cap concurrency per node — learned the hard way (2026-06-22).** Pointing ALL
>   builds at one node (BigBoy) and running **6 concurrent heavy `mde-workbench`
>   (libcosmic GUI) builds** drove `.52` to **load 49 + disk → 3.6 GB free**; builds
>   stalled, agents hung waiting on builds that never finished, and several otherwise-good
>   units became "duds" purely from host exhaustion (their code compiled — only the host
>   couldn't link/test under contention). A heavy GUI build wants ~1 core + GBs of `target`;
>   keep **≤2–3 heavy builds per node** and genuinely spread across `.50/.51/.52`. When a
>   node is saturated, salvage a stuck agent's uncommitted code by building it on a FREE
>   node (`MCNF_BUILD_HOST=172.20.0.51`), then commit + cherry-pick. Killing a farm agent
>   leaves its remote `cargo`/`rustc` orphaned (the SSH child keeps building) — they
>   self-clear on completion; a blanket remote `pkill` is (correctly) classifier-blocked on
>   shared infra, so prefer not to over-spawn in the first place.
> - **Parallel mutating agents:** spawn with `isolation:"worktree"` (no code
>   cross-contamination). They cut from the **master tip**, so tell each to
>   `git reset --hard <current-work-tip-sha>` first; have each commit its **disjoint**
>   files + report the SHA; then **cherry-pick** the SHAs onto the work branch (clean,
>   since disjoint). **Clean up** the agent worktrees afterward (`git worktree remove`) —
>   their `target/` dirs fill the dev-host disk fast.

The development toolchain and build environment are documented **once**, in
[`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md) — **read it before building
or provisioning; do not relearn it.** If it has drifted, fix that file, not your
memory. The load-bearing facts (full detail + the gotchas index live in the doc):

- **Two build surfaces.** (1) The **local dev host** (`172.20.145.192`, Rocky 9.8)
  builds the whole workspace incl. the GUI — but its **gcc 11.5 rejects `mold`**, so
  build with `RUSTFLAGS="-C link-arg=-fuse-ld=gold"`. (2) The **build farm** (two
  Fedora VMs `172.20.0.50`/`.51`, gcc 15 + mold) for offloaded/parallel builds, gates,
  and RPM cuts — drive it with `install-helpers/xcp-build.sh` / `farm.sh`.
- **Rust pin `1.94.0`** (`rust-toolchain.toml`); MSRV floor `1.85`. `rustup` required.
- **EL9 prereq trap:** `opus-devel` is in **CRB** (`dnf --enablerepo=crb install opus-devel`),
  not the default repos — the single most-rediscovered dependency.
- **The farm is Infrastructure-as-Code** (DEVOPS-SUBSTRATE): `infra/tofu/` (OpenTofu +
  Xen Orchestra) builds the VMs from the `MDE-VM-golden` template; `infra/ansible/`
  installs the toolchain. `tofu apply` rebuilds the farm from code. Secrets
  (`/root/.mcnf-xo-token`, the mesh key) are off-repo.
- **Build PLATFORM direction** (`docs/design/build-platform.md`, locked 2026-06-22):
  the **GitOps reconciler on a timer** is the canonical build lane — builds happen
  because the worklist changed (an `@farm:{…}` tag on a task), **no AI in the build
  loop**; AI spends tokens only on design + failure-triage. **Shared `sccache`** is
  the build-speed lever. Correctness is proven by an **internal** test pyramid — L0
  build+unit on every change (blocks green), L1 install + L2 feature + L3 stability
  (soak/chaos/reboot) **nightly + on-demand, never blocking** — run on a
  **snapshot-reset VM pool** from `MDE-VM-golden`. The 5 FARM-AUTO capabilities are
  the substrate; the reconciler is the default.

---

*Heritage: the pre-pivot identity lives in the archived
[MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation) repo, whose
labwc-era desktop this pivot end-of-lifes.*
