# MCNF — Mackes Cosmic Nebula Fedora — Governance & Architectural Locks

> **Identity:** *MCNF (Mackes Cosmic Nebula Fedora) is a secure, no-fixed-center
> workgroup mesh and its native-Rust Carbon GUIs, running on stock Fedora-Cosmic.*
> This document is the platform's identity + the architectural locks (§0–§9) and the
> **process locks** (§10 — repeatable design → build → verify → remediate). When a lock
> conflicts with prose elsewhere, the **newer lock wins**; authority ranks
> **Memory > this file > design docs > worklist body**. A lock reopens only on a
> concrete new symptom + a dated superseding entry in `docs/DECISIONS.md` (§10).
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
- **Substrate split (SUBSTRATE-V2, lands 11.0 "Winter-Is-Coming"):** mesh
  **coordination** (leader election, the peer directory, health) lives in **etcd**
  over Nebula (strongly consistent, on the anchor nodes); **files** sync via
  **Syncthing** full-mesh on `/mnt/mesh-storage` (a plain dir, no FUSE). LizardFS
  (and MooseFS/Gluster/Ceph) are **retired/forbidden** — the single-mount-carries-
  everything model caused "mount down = mesh down". `~/Local/` is never mesh-synced.
  *(Until 11.0 ships, the running fleet is still on LizardFS; design:
  docs/design/substrate-v2.md.)*
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

**Visual-confirmation gate lifted (2026-06-11) — RE-INSTATED as an automated gate
(2026-06-21, §10 B8).** The 2026-06-11 lift was conditioned on visual confirmation
being hardware-gated + manual. Self-hosted CI now makes **deterministic headless
capture** feasible, so render correctness is gated again by an **automated pixel-diff
visual regression** (§10 B8) — not by manual `/preview`. The token discipline (§4) and
palette/component tests still stand underneath it. See `docs/DECISIONS.md` for the
superseding rationale.

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

## §10 — Process Locks: repeatable Design → Build → Verify → Remediate (2026-06-21, PROCESS survey Q1–Q57)

The platform's **process** is locked the same way its architecture is. Every change
runs through the gates below; each is **binary pass/fail** and cited by ID (`D1`,
`B2`, `V3`, `R4`). The gate is **executable, not prose** — a single
`install-helpers/verify-gates.sh` *is* the gate; the pre-commit hook and CI both run
it verbatim, so the three can't drift (design + full Q-lock map:
`docs/design/process-governance.md`). The goal is to stop effort going to problems
that didn't need solving: nothing speculative, nothing untracked, nothing re-litigated.

**Standing operating posture — `no-flinch` (operator directive 2026-06-21).** This skill
is **standard guidance for every operation** under §10, not just autonomous drains. The
failure mode it counters: gravitating to work with a fast/clean success signal and routing
around work whose feedback is **slow, fuzzy, gated, or expensive** — then dressing it up as
"efficiency." Nothing here is genuinely *hard*: "hard" is a dishonest word for slow or fuzzy
feedback. The rules bind every gate above: **(1)** pace + priority are the operator's — never
slow the loop or reclassify work as "lower-value tail" on your own judgment; **(2)** "harder
for me" ≠ "less urgent for them"; **(3)** *a gate is a task, not an excuse* — "XO isn't up" /
"needs a token" → stand it up / wire it (provisioning is pre-authorized, §10 environment);
**(4)** fix the feedback loop (slow build, missing harness) instead of avoiding it; **(5)**
**finish `[✓]` over pile `[>]`**; **(6)** accept slower/fuzzier verification when the work
needs it. The tell: if you're about to defer the gated/slow/infra unit for an easy one —
that's the flinch; do the avoided thing instead. (Skill: `.claude/skills/no-flinch`.)

**Entry (before any work).** **E1 — tracked:** a worklist task exists. **E2 —
symptom-backed:** a concrete observed trigger, or (for preventive work) an existing
**§3/§8 control** it enforces — no speculative work. *Trivial fixes* (typos, dead
imports, fmt/clippy nits) skip E1/E2 but still pass the gate.

**Design (D)** — required when a change **adds a crate, crosses ≥2 crates, or adds a
surface** (localized changes go straight to a task):
- **D1** A `docs/design/<epic>.md` exists with all mandatory sections: Locks table ·
  Architecture · Acceptance (each runtime-observable) · Risks · Out-of-scope ·
  **Non-Goals** (what we pre-commit *not* to build).
- **D2** Every ≥3-option fork is decided + locked by the agent as **source of truth**,
  rationale recorded.
- **D3** Every actionable row carries a worklist **task ID** and every task back-links
  the doc; `lint-design-worklist-link.sh` proves the map is total.
- **D4** Decomposition: an **epic = a releasable capability**; a **task = the smallest
  unit that independently passes the full gate**; tasks carry `depends-on: <task-id>`.

**Build + Test (B)** — **every commit** is green via a **blocking pre-commit hook**
running the full gate (no red commit ever exists):
- **B1** `cargo build --workspace --locked` clean.
- **B2** `cargo clippy --workspace --all-targets` clean (crypto/unwrap deny-level).
- **B3** `cargo fmt --all -- --check` + shell lint (shellcheck/shfmt); IaC linters
  (ansible-lint, `tofu validate/fmt`, yamllint) phase in with the infra epic.
- **B4** Tests green; **test-first** — the failing acceptance/integration test predates
  the implementation (features as well as fixes). mackesd runs parallel once **EFF-18**
  (the env-race) is fixed at the root.
- **B5** Lint gates: mesh-boundary (§6), carbon-tokens (§4), bus-names (§2),
  libcosmic-rev, no-crates.io-iced, **design-worklist-link** (D3).
- **B6** `cargo deny check` (advisories/licenses/bans/sources).
- **B7** **Diff-coverage ≥ 90 %** on changed lines (not whole-repo).
- **B8** **Pixel-diff visual regression** on the headless preview gallery (deterministic:
  pinned fonts + software rasterizer + fixed resolution); golden images **re-blessed in
  the same commit**, diff visible in git. *(Supersedes the §7 visual-confirmation lift —
  see `docs/DECISIONS.md`.)*
- **B9** Operator-visible behavior change updates its **docs in the same commit**
  (ADMIN / `docs/help` / CHANGELOG).
- **B10** Commit message carries **task ID + the entrypoint made reachable + why-not-what**.

**Verify (V)** — before a task flips to `[✓]`:
- **V1** Runtime-reachable: the **concrete entrypoint is named AND a test drives that
  path** (not merely claimed).
- **V2** Cross-crate / new-surface features carry an **integration test across the real
  path**.
- **V3** A new surface has **CLI parity + a CLI test** (GUIs are renderers; headless-operable).
- **V4** Mesh-level features pass the **multi-node gate on real XCP-ng VMs at 3 LH + 3
  peers** (snapshot-reset pool); the full **3 LH + 9 peer** envelope + a golden-image
  rebuild run at release.
- **V5** No stubs: no `todo!()`/`unimplemented!()`, no stub match arms, no `pub mod`
  with zero external refs, no `demo_data`/mockups (§7).

**Remediate (R)**:
- **R1** Defects follow **reproduce → failing test → fix → gate**; the regression test stays.
- **R2** Dead/mock/incomplete code defaults to **REMOVE** unless a worklist task commits
  to finishing it on a date.
- **R3** **No in-flight detours** — an unrelated defect Y is filed as its own
  symptom-backed task; X finishes. A blocking Y marks X `[!] Blocked`.
- **R4** Transitional **dual-path code is allowed only with a tracked cutover task + a
  removal date**.
- **R5** Rollback is **fix-forward only** (never revert; the new fix rides R1).
- **R6** Each **incident** yields a regression guard **and** a one-line governance lock
  capturing the invariant.
- **R7** **Stop-and-escalate** when the approach has been substantially rethought twice
  without passing; the escalation is **the symptom + an `AskUserQuestion` choice**.

**Process meta.**
- **Authority / reopen:** every lock (§0–§10) reopens **only** on a concrete new symptom
  + a **dated superseding entry in `docs/DECISIONS.md`**; agents may edit this file under
  that same rule. Newest lock wins.
- **Integration:** **direct commits to master**; parallel work runs in **per-agent git
  worktrees**, merges serialized (one fast-forward at a time, re-gated on conflict).
- **Visibility:** the worklist status legend (`[ ] [>] [✓] [!]`) is the state signal; a
  generated **dashboard** surfaces it; **done epics archive** to `docs/WORKLIST-archive.md`.
- **Notifications:** push only **escalations + release-ready**; all else is pull (dashboard).
- **Releases:** **milestone-based** — an epic reaching full §7-completion is a release
  candidate; the RPM cut stays **operator-gated** (`/release`).
- **Environment:** airgapped **dev** — no production; full machine control. CI is
  **self-hosted Forgejo Actions** on the XCP-ng fleet (GitHub stays canonical for the
  release pipeline, Forgejo pull-mirrors); infra is **OpenTofu/Terraform (Xen Orchestra)
  + Ansible**; secrets ride **etcd + age over Nebula** (D-W1).
- **Rollout:** §10 is active now; the infra-dependent gates (self-hosted CI, B8 visual,
  V4 real-VM) **phase in as the infra epic lands** — and **building that infra is the
  first §10 epic** (the process dogfoods itself).

---

*Heritage: the pre-pivot identity lives in the archived
[MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation) repo, whose
labwc-era desktop this pivot end-of-lifes.*
