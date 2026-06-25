# PLANES — the five-plane Workbench console

**Date:** 2026-06-09 · **Survey:** 100 questions (W1–W100, 8 rounds + 4 conflict
pre-locks) + 10 hero-image questions (H1–H10) + standing directive **D-W1** ·
**Status:** locked, lifted into `docs/WORKLIST.md` (### PLANES) · governance: new §9

The operator's tree — Host Agent/Node Daemon, Controller, Provisioning Plane, Network
Plane, Distributed Plane — mapped onto the platform and placed in the Workbench as its
top-level mesh IA. **D-W1 (standing): reuse mesh tooling first, Red Hat best practices
second.** Greenfield: no legacy or tech debt carried (operator, W78 reply).

> **Note (post-SUBSTRATE-6):** the locks below predate the substrate split and say
> "state on LizardFS" / "LizardFS replicates." **LizardFS is removed.** Coordination
> (the Controller plane's leader election + peer directory + health) now runs on
> **etcd**, and shared files (fleet state dirs, job templates, mirror trees) sync
> over **Syncthing** on a plain `/mnt/mesh-storage` dir. Read "LizardFS" in the
> locks/prose as today's etcd+Syncthing substrate; the W62 `file://`-mirror coupling
> now keys off Syncthing folder health, not a FUSE mount.

## Constitutional locks (→ AI_GOVERNANCE §9)

| # | Lock |
|---|------|
| W1 | **No RBAC. Access to the mesh IS the control plane** — a valid mesh cert is the authorization; the desktop is the operator (§8) |
| W2 | **3 roles (§5) + capability tags** — hop / execution / headless are orthogonal, GATING (W84) tags |
| W3 | **The Controller is a plane, not a place** — coordination state on etcd + file state synced by Syncthing (was LizardFS, retired SUBSTRATE-6), any node hosts it, the FPG election coordinates |
| W21 | Remote execution = **typed Bus verbs + signed job bundles only. No raw shell channel, ever** |
| W88 | **All fleet state = TOML/YAML dirs on the Syncthing-synced `/mnt/mesh-storage` (was LizardFS) + typed mackesd Bus verbs; GUIs are renderers** (CLI parity, W27) |
| D-W1 | **Mesh tooling first, Red Hat second** — FPG/Bus/etcd+Syncthing (was LizardFS)/Nebula before new components; Ansible/nmstate/firewalld/kickstart/createrepo where the mesh has no native tool |

## The IA (W4–W16)

Nav top-to-bottom: **Peers (Front Door) · This Node · Controller · Network · Fleet ·
Provisioning · Desktop** (personal panels grouped last). Full tree from day one with
guided empty states on unbuilt panels (W16, L3 pattern). Deep-link ids: clean break
(W11). Existing panels absorbed (W4): Mesh Services→This Node/Health · Drift→
Controller/Remediation · Fleet Revisions→Controller/Config · Playbooks→Controller/Jobs
· Mesh Control→Controller (own entry, W52) · fleet health→Fleet plane (W14) · Peers =
Controller/Inventory, one component two doors (W7).

## Plane locks

### This Node (W9, W17–W28)
Always the local box (W9). **Registration**: full cert lifecycle + invites + token
minting in one panel (W17/W18); fingerprint as hex + word-pair phrase (W25); tags
viewed here, edited in Provisioning (W26). **Inventory**: PeerProbe rendered, no new
collectors (W19). **Health**: ENT-7 doctor + service controls + Netdata alarms (W20);
self-restart via systemd (W24). **Config**: revision vs newest + apply log + Reconcile
now (W22); version + repo source + update-now via typed job (W28). **Logs/metrics**:
journald mesh-unit view + Netdata strip (W23). **Full CLI parity** for all six (W27).

### Controller (W29–W52)
**Jobs**: job = Ansible playbook ref + vars + targets (W29), schedules in v1 (W30)
fired by the FPG leader (W35); targets = tags/roles/peers resolved at launch (W31);
the TARGET runs it locally as a signed bundle — no push-SSH (W32); Syncthing-synced
`jobs/templates/` + `jobs/runs/<id>/` under `/mnt/mesh-storage` (W33, was LizardFS); fleet-parallel, serial per node (W34);
history runs→targets→output (W36); Run-once (W37); form + read-only YAML, playbook
authoring external (W38); failure-only alerts (W39); Playbooks panel absorbed (W40).
**Remediation**: drift-type → template map with var bindings (W41); per-plan auto
flag default off (W42); Drift panel folds in (W13). **Audit**: scope = security +
jobs/remediation + config/policy + lifecycle ops (W43); timeline viewer + verify-chain
(W44); **retention 72 hours** (W45 — a rolling forensic window, operator lock).
**Policy**: declarative TOML assertions over replicated data (W46/W47); eval on
input-change + hourly leader sweep (W48); **violation = drift event** — one
remediation pipeline (W49); core pack ships enabled (W50); report default, enforce =
opt-in auto-plan binding (W51). **Fleet logs** search (OBS-5) lives here (W15).

### Network (W65–W80)
**Full nmstate** desired state via NetworkManager (W65/W66), stored **in the
BaselineSpec** (W67) — one revision stream. Panel = desired-vs-actual diff (W68).
**firewalld zones; overlay interface = trusted zone** — cert possession is the
perimeter (W69/W70); revocation stays Nebula-blocklist-only (W71). **VPN panel**:
Nebula topology as fleet state + external client profiles (never mesh transport, §1)
+ **site gateways: hop-tagged nodes advertise subnets AND serve as full exit nodes**
via unsafe_routes (W72/W73). **Mesh DNS**: mackesd feeds the roster into
systemd-resolved per-link — `<host>.mesh`, no server, no center (W74/W75). Routing
otherwise display-only (W76). **Applies are all-at-once + nmstate checkpoint
auto-revert**: post-apply the node must re-reach a lighthouse + one peer or it reverts
(W77/W78 — the self-test IS the rollback trigger). Validation suite v1 = overlay
reachability, run post-apply + nightly + manual; failures → drift (W79/W80).

### Fleet (W81–W87)
A **rollup dashboard — a lens, not a config surface** (W81): groups by role + tag,
cards show health + presence (W86), live map (PD-7) centerpiece; drill-down selects
into the Peers directory (W87). **Tags v1: hop, execution, headless** (W82; builder/
mirror deferred — W54's build jobs target `execution` nodes until then). Tag authority:
any enrolled surface, audit-logged (W83). **Tags GATE**: untagged duty is refused
(W84). headless = GUI units off via systemd presets, full agent, one RPM (W85).

### Provisioning (W53–W64) — lands after PKG core (W64)
**Images**: all four kinds — install ISO + kickstarts, VM golden images, container
images, USB writer (W53); built by jobs on the mesh (W54, → execution tag per the W82
reconciliation); stored as versioned dirs + TOML manifests on the Syncthing-synced `/mnt/mesh-storage` (W55, was LizardFS).
**Profiles**: role + tags + kickstart fragments + join-token slot, TOML, form-edited
(W56); one image carries all profiles, **boot-menu choice** at install (W57);
**auto-join**: single-use bearer baked in, firstboot enrolls + pins + tags (W60).
Bootstrap v1 = USB/ISO only, PXE deferred (W59). **Node roles panel** (role pins +
tag editor) lives here (W58). **Mirrors**: the magic-mesh GitHub-hosted dnf repo (W61, ex-COPR); **every node serves
itself** — dnf reads the Syncthing-synced `/mnt/mesh-storage` mirror via `file://` baseurl, no HTTP tier (W62, was the LizardFS mount);
sync = scheduled job, one puller, Syncthing replicates the tree (W63, was LizardFS).

## Delivery (W89–W100)

Per-plane Bus prefixes (W89: `action/jobs|remediate|policy|audit|netstate|provision/*`).
Engines = mackesd workers gated **Server rank 1+** (W91); UI = workbench panels; no new
crates (W90). **One mega-epic** (W92), order **IA → Node → Controller → Network →
Fleet → Provisioning** (W93). **FPG is a hard prerequisite** (W94). Existing
plane-relevant ENT/OBS tasks **re-home with tombstones** (W96). Standard CI gates +
OBS-2 convergence tests for fleet-state engines (W97). Codename **PLANES** (W98).
**Ship order: PEERS → FPG → PLANES** (W100).

**Out of scope (W99):** multi-mesh federation · cloud/elastic nodes · non-Fedora
agents · human multi-tenancy.

## Hero images (H1–H10)

Every section whose primary engine is an external project carries a **hero**:
**line-art outline** originals (H2/H5 — homages, zero trademark exposure) in the
**header band, right-aligned, 96–128 px** (H3/H4), captioned **NAME + live installed
version** (H8), **hover → stack card** (project, version, license, platform role, docs
link — H9), **always rendered** ("not installed" honest caption when absent, H10).
Set (H1): Ansible, etcd, Syncthing (the SUBSTRATE-6 substrate, replacing the
retired LizardFS hero), Nebula, Fedora, Netdata, Podman, libvirt/KVM, Cosmic,
systemd, Remmina, PipeWire, **rustls (TLS)**, VPN/tunnel tech. Assets: SVGs compiled
into **mde-theme** behind a typed `Hero` enum (H6, §4 single-source); stroke color is
a new **`hero_stroke` token**, palette-tested (H7).

## Risks

- **W45 72-hour audit retention** inverts the usual audit posture — chain verification
  only ever covers 3 days; flagged at lock time, operator confirmed.
- **W77 all-at-once network applies** put the whole fleet one bad revision from
  isolation; the W78 auto-revert self-test is load-bearing — it ships in the same task
  as the apply path, never after.
- **W73 full exit nodes** = routing-table surgery on clients + NAT on hops; the
  validation suite must cover the exit path before the toggle ships.
- **W62 file:// mirrors** couple updates to the Syncthing file-plane health (was
  LizardFS): a storage outage blocks dnf. Upstream-fallback stays in the .repo
  (cost: W61 scope only).
- **W84 gating tags** are a behavior change for existing workers when tags arrive —
  the tags substrate must land before any gate flips (sequenced inside the epic).
- **W16 full tree** renders ~20 guided empty states for months — each must name its
  worklist item honestly, or the console reads as vaporware.
