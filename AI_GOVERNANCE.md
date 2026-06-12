# Magic Mesh — Governance & Architectural Locks

> **Identity:** *Magic Mesh is a secure, no-fixed-center workgroup mesh and its
> native-Rust Carbon GUIs, running on stock Fedora-Cosmic.* This document is the
> platform's identity + the architectural locks. When a lock conflicts with prose
> elsewhere, the **newer lock wins**; authority ranks **Memory > this file >
> design docs > worklist body**.

This rewrites the MackesWorkstation governance for the **E11 "Magic Mesh" pivot**.
The labwc/Win-era desktop is end-of-life; Cosmic owns the desktop. The locks below
are what survives the cutover, restated for the Cosmic context — the
shell/compositor/era-theme/desktop-surface locks are **retired**.

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
- **LizardFS** is mesh storage (not Gluster, retired wholesale). `mackesd` owns the
  mount; XDG dirs replicate topology-aware; `~/Local/` is never mesh-mounted.
- **`mackesd`** is the supervised control-plane daemon — owns the worker pool, the
  mesh/CA state, the KDE-Connect host, and the SQLite store. One leader per the
  shared lockfile; owns writes.

## §2 — The Bus (not D-Bus)

- Surfaces and `mackesd` communicate over **`mde-bus`** (internal pub/sub). MDE-
  private D-Bus is retired; the only `dbus`/`zbus` use is **FDO interop**
  (`org.freedesktop.*` — Notifications host, session FDO). A lint gate forbids new
  MDE-private bus names.
- Bus calls **degrade gracefully** with no mesh / no peers: cached state, Bus
  timeouts, never panic.

## §3 — Security: maximum crypto

The fabric is pinned to the strongest interoperable values, asserted by config
tests: **Ed25519** node identity · **AES-256-GCM** / **ChaCha20-Poly1305** ·
**XChaCha20-Poly1305** CA backup · **RSA-4096** KDC device identity. Enrollment
uses max key complexity. No OpenSSL — **rustls** throughout.

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
  motion) are **single-sourced in `mde-theme`** and guarded by a lint gate. No raw
  hex / scattered metric literals outside the token modules.
- **Pure-Rust toolkit:** iced 0.14 (wgpu) + cosmic-text (no FreeType), rustls (no
  OpenSSL). GUIs ship as Cosmic apps + a native cosmic-applet; Cosmic draws the
  panel/decorations, Magic Mesh draws its client areas.

## §5 — Distribution & roles

- **Cosmic provides the desktop.** Magic Mesh integrates *into* it: a cosmic-applet
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
  ≤ 8 peers** (the `ca/sign.rs` cap). Within that envelope the bar is **reliable + operable +
  documented** — once the ENTERPRISE + PKG epics land, the platform is held to production-grade for
  that scale, and `DISCLAIMER.md` is repositioned accordingly (no longer "not for production"). Going
  beyond ≤ 8 peers / HA-multi-lighthouse / multi-tenant is **out of scope** for this identity.
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
- **The Controller is a plane, not a place.** No-fixed-center holds: control state
  lives on LizardFS, any node hosts the surfaces, the elected leader only
  coordinates (schedules, sweeps). (W3)
- **Remote execution is typed verbs + signed job bundles only — no raw shell
  channel, ever.** Jobs are Ansible playbooks the *target* runs locally; no
  push-SSH. (W21/W32)
- **One state doctrine:** every plane's durable state = TOML/YAML dirs on LizardFS +
  typed `mackesd` Bus verbs; GUIs are renderers; every surface has CLI parity. (W88/W27)
- **Mesh tooling first, Red Hat best practices second** (standing directive D-W1):
  FPG/Bus/LizardFS/Nebula before new components; Ansible/nmstate/firewalld/
  kickstart/createrepo where the mesh has no native tool. Greenfield — no legacy
  carried.
- **Hero images:** sections backed by an external project carry a Carbon-compliant
  line-art hero (original homages, `mde-theme`-compiled, `hero_stroke` token). (H1–H10)

---

*Heritage: the pre-pivot identity lives in the archived
[MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation) repo, whose
labwc-era desktop this pivot end-of-lifes.*
