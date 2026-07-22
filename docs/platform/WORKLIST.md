# Platform Worklist

Authoritative active worklist after the 2026-07-16 reconciliation.

Historical source rows were moved out of this file and preserved at:

- `docs/worklist-archive/2026-07-16-platform-worklist-pre-reconcile.md`
- `docs/worklist-archive/2026-07-16-platform-worklist-marker-index.tsv`
- `docs/worklist-archive/2026-07-16-reconciliation-archive.md`

The reconciliation report and execution order live at
`docs/platform/WORKLIST-RECONCILIATION-2026-07-16.md`.

This file is the only active platform worklist. Other roadmaps, design notes,
review ledgers, and operator queues are evidence sources, not parallel trackers.
When an item is completed or retired, move it to the archive with a disposition
instead of leaving closed work in this file.

## Fold-in - 2026-07-20 (master planning-line reconciliation)

The diverged `origin/master` planning line (5 worklist-only commits, forked at
`756eca42`) was merged into this history. Master had rewritten `docs/WORKLIST.md`
into a from-scratch "local-first virtualization + containers" plan while this
branch implemented that same direction across 266 commits. The merge keeps ONE
tracker (this file, via the `docs/WORKLIST.md` pointer) and preserves the
planning-line content without loss:

- **`docs/worklist/LOCAL-FIRST-VIRT-CONTAINERS.md`** - the local-first re-plan
  (LOCAL/PERF/NET/DEVICE/CTR governing outcomes). Mapping to real status: the
  "remove OpenStack, standardize on libvirt+QEMU/KVM, Podman+Quadlet containers"
  spine is already **done** under `WL-ARCH-001` (code-complete); the workstation
  surface for it is in flight under `WL-ARCH-006` (Workloads cockpit); the
  remaining net-new items (LVM thin-pool lifecycle, tiered GPU passthrough,
  low-latency audio core scoring, per-host routed VM/container subnets) are
  hardware/live-fleet-gated and stay parked with that gate named.
- **`docs/worklist/EGUI-SHELL-VISUAL-REFINEMENT.md`** - a net-new active GUI epic
  (`UI-VIS-101..145`, shell design-token + component-refinement sweep). Tracked
  here as available for a future `/polish` fan-out; NOT part of the current
  WL-ARCH-006 drain scope.

## Drain reconciliation - 2026-07-19 (authoritative)

An 8-agent reconciliation (`wf_924f2a46-283`, 929k tokens, file:line evidence per
epic, run against `agent/browser-enterprise-hardening` @ `b999251e`) re-verified
all 43 epics against actual code. Full evidence + gates:
**`docs/platform/DRAIN-RECONCILIATION-2026-07-19.md`** (authoritative; the per-epic
`Status:` lines below defer to it where they disagree).

Disposition of all 43:

- **Done - closed & archived (8):** WL-ARCH-005, WL-CRIT-002, WL-CRIT-005, WL-FUNC-004,
  WL-PERF-001, WL-PERF-003, WL-RUN-001, WL-RUN-005. Verified complete on real code
  paths; moved out of this active file to
  `docs/worklist-archive/2026-07-19-drain-closed.md` per the archive-on-close rule.
- **Drained this session - code landed + tested + pushed (9):**
  WL-BUILD-003 (`ed456387`, rollback verb+drill+runbook; secret-scan sub-item
  deferred per operator), WL-FUNC-003 (`39d4ddba`, two-store convergence fixture),
  WL-RUN-002 (`0f15faa2`, reconcile/drift/bus-error counters), WL-PERF-002
  (`643ac7d7`, live-VDI repaint; optional live-seat wake proof remains),
  WL-DOC-001/002/003 (`ad44f1ed`, supersession banners+lint / NEEDS-OPERATOR re-key
  / stewardship lifecycle), WL-TEST-001 (`19bc4559`, OpenStack create→verify→delete
  harness — live *run* blocked on a farm OpenStack endpoint that does not exist yet),
  WL-SEC-004 (`3d422e07`, seated-user arm/disarm consent publisher). Each built +
  targeted-tested green on the farm.
- **Held for operator scoping - Epic-sized, NOT one-shot autonomously (6):**
  WL-ARCH-003 (BusReader migration of all reader surfaces), WL-ARCH-004 (unify ~136
  imperative spawn sites into one declarative registry), WL-SEC-002 (cross-mesh
  federation enforcement + harness), WL-FUNC-008 (unified ServiceRecord aggregator -
  whole deliverable unbuilt), WL-RUN-006 (router mutation fast-follow), WL-UX-005
  (Start-Menu dedup + peer-app remote exec). Deliberately left for you to sequence -
  each is a multi-PR architectural change, not a clean single-commit drain.
- **Seat-visual proof (1):** WL-FUNC-006 - all code acceptance met; only a live `.15`
  bottom-rail screenshot remains (folded into the live-verify list; the shell is
  deployed on `.15`).
- **Needs operator decision (3):** WL-ARCH-002, WL-FUNC-005, WL-UX-003 - a named
  dependency is an unmade design decision (see ledger).
- **Park-blocked (16):** WL-ARCH-001, WL-BUILD-001, WL-BUILD-002, WL-CRIT-001,
  WL-CRIT-004, WL-FUNC-001, WL-FUNC-002, WL-FUNC-007, WL-FUNC-009, WL-FUNC-010,
  WL-RUN-003, WL-RUN-004, WL-SEC-001, WL-SEC-003, WL-TEST-002, WL-UX-001 - each
  gated on hardware, a live fleet, external account, or signing/release authority.

**Drain executed 2026-07-19: 17/43 fully resolved (8 archived-done + 9 landed),
1 seat-visual (FUNC-006), leaving 6 Epic-sized held for operator scoping + 19
(3 decision + 16 park) that genuinely need the operator (hardware, live fleet,
external account, signing/release authority).** The autonomous drain is complete to
its ceiling; the remaining 25 are honestly categorized with their gate named -
beta-readiness needs them *parked-with-a-gate*, not *done*.

**Post-reconciliation operator addition:** WL-FUNC-011 was added after the
43-epic 2026-07-19 drain audit. It is outside that historical count and evidence
ledger; the audit's totals remain a snapshot of the worklist it evaluated.

**Update 2026-07-20:** original 43 drained to 3 active — **WL-ARCH-001** (CODE-COMPLETE; OpenStack removed + OpenTofu/Ansible/libvirt backend + iac/ workspace; only Phase D live smoke, operator/hardware-gated), **WL-FUNC-011** (PARITY-COMPLETE; full Communications stack + all 6 modes live; only the one-big cutover, operator-gated), **WL-RUN-003** (held). New epic **WL-ARCH-006** (Workloads cockpit) added below as WL-ARCH-001's surface successor (21-unit farm fan-out; **CODE-COMPLETE 2026-07-20 — all 21 units landed + workspace-green + pushed `bae119e6`; only live-seat smoke + mirror rich-payload decode remain**).

## Status Vocabulary

- `Remaining` - valid unfinished work that can proceed.
- `Blocked` - valid unfinished work that needs a named live resource, operator
  action, hardware, external account, or release gate.
- `Needs clarification` - valid concern, but the next implementation cannot be
  safely specified from current repo evidence alone.

## Operator Decisions - 2026-07-16

These decisions refine acceptance and sequencing for the active items below.

- WL-CRIT-004: use the existing DO Spaces DR bucket; an agent may run the exact
  audited DR export command; restore proof defaults to rebirthing a fresh control
  node.
- WL-CRIT-005: perform a hard substrate cut anytime; the live lighthouses are
  not carrying production; full quorum mutation is allowed with rollback; retire
  LizardFS immediately.
- WL-SEC-001 and WL-BUILD-001: use `.15` for both fresh-node enrollment and
  Workstation ISO wipe/reinstall; preserve nothing on `.15`. `.138` stays spare.
- WL-SEC-003: first proof is two authorized nodes decrypting the existing sealed
  DO token.
- WL-SEC-004 and WL-FUNC-002: bundle phone remote-input authorization/indicator
  with the same KDC phone-authenticator wave; use an existing paired phone and
  require phone approval plus successful third-party login.
- WL-BUILD-002: start with shared sccache backend as the best-practice first
  slice.
- WL-BUILD-003: defer secret-scan gates until after DR/ISO work.
- WL-RUN-003: prove lighthouse retirement first, but acceptance is a full
  add-retire-add cycle.
- WL-RUN-004: first live media target is failover across existing live
  lighthouses.
- WL-FUNC-001: first protected-media proof is a YouTube DRM/media capability
  page on `.15`.
- WL-FUNC-003: sync system-bookmark-manager bookmarks before other Browser sync
  state.
- WL-FUNC-004: Browser download manager comes before other power tools.
- WL-FUNC-005: first unified-search slice is home-directory filenames plus
  metadata.
- WL-FUNC-007: first proof is local video playback from an existing sample on
  the seat.
- WL-RUN-001: implement real take-action repair rather than only renaming the
  observe-only path.
- WL-RUN-002: wire worker-restart counters first.
- WL-RUN-005: verify paired phones as the first non-PC Device Manager source.
- WL-RUN-006: keep firewall commit-confirm active.
- WL-ARCH-001/WL-ARCH-002/WL-TEST-001: continue Construct Cloud in parallel with
  substrate work; finish Compute instance verbs/forms first; live smoke creates
  and deletes a nano server instance.
- WL-ARCH-003: begin shared Bus/Persist seam work soon.
- WL-ARCH-004: split worker registration/decomposition into smaller
  worker-family tasks before implementation.
- WL-PERF-001: optimize SPICE dirty rectangles first.
- WL-PERF-002: verify VDI frame wake behavior first.
- WL-UX-001: pass/fail is screenshot/pixel proof on `.15`.
- WL-UX-005: track the Start Menu / Front Door launcher overhaul as one epic;
  keep WL-UX-001 scoped to bottom-bar/start/tray live proof and WL-FUNC-005
  scoped to shared search/index plumbing.
- WL-DOC-001: clean current operator docs first:
  `docs/help/install.md`, `docs/help/node-setup.md`,
  `docs/BUILD-ENVIRONMENT.md`, and `docs/ops/promotion-pipeline.md`.
- WL-DOC-002: merge `docs/NEEDS-OPERATOR.md` fully into this active worklist;
  it should not remain a separate queue.
- WL-DOC-003: require an archive entry for every closed item.
- WL-TEST-002: first harness target is existing live lighthouses; full quorum
  mutation is allowed with rollback.

## Critical Correctness And Data-Loss Risks

## Security

## Build, Installation, And Deployment

## Core Architecture

### WL-ARCH-001 - Remove OpenStack; OpenTofu + Ansible IaC workspace for all cloud operations

- Status: Blocked
- Progress (2026-07-20): CODE-COMPLETE. Phase A delete (222e1980, -19k LOC) + Phase B OpenTofu/Ansible/libvirt backend + mackesd cloud worker (1dad89d2) + Phase C recreated six-mode iac/ cloud-ops workspace (19e0089038 -> c2a3f76d) all landed + tested green; zero OpenStack in production code. Only Phase D remains: the live local-libvirt provision+configure smoke (MDE_CLOUD_APPLY=1 on a libvirt host) — operator/hardware-gated. NB: the coarse Phase-C six-mode iac/ is being reenvisioned by WL-ARCH-006 (Workloads cockpit — delivery-type x mesh placement); WL-ARCH-006 is the surface successor over this same OpenTofu+Ansible+libvirt backend.
- Priority: P1
- Complexity: Epic
- Problem: Construct Cloud is coupled to OpenStack (Nova/Heat/Keystone/Kolla,
  state/openstack/* mirrors, cloud_plane.rs/console/front_door OpenStack copy).
  Operator directive 2026-07-19: REMOVE ALL OpenStack and rebuild cloud operations
  on OpenTofu + Ansible against local libvirt, with the IaC workspace recreated as
  the single surface for every cloud operation.
- Required outcome: Zero OpenStack anywhere (workers, surfaces, mirrors, docs,
  deps). A recreated `iac/` workspace drives ALL cloud operations end to end via
  OpenTofu (provision) + Ansible (configure) against local libvirt/KVM, and can
  provision + configure a workload with no OpenStack code present.
- Decided stack (operator 2026-07-19, Red Hat / cloud-native standards):
  1. **Provision = OpenTofu** (declarative; replaces Heat/HOT + Nova verbs).
     libvirt provider for local VMs; networks + images declared as Tofu resources.
  2. **Configure = Ansible core** (playbooks/roles; replaces OpenStack config).
     Ansible roles drive the EXISTING mackesd-written `/etc/mackesd/site.yml`
     convergence (boot-durable, reuses the SEC-001 join path).
  3. **VM/workload backend = local libvirt/KVM** (E12 local-first; no external cloud).
  4. **Images = bootc image-mode + osbuild/image-builder** (extends packaging/bootc/).
  5. **Containers = Podman + Quadlet** systemd units (replaces Kolla), Ansible-managed.
  6. **Tofu state = etcd-backed** (mesh-native; consistent with infra/tofu/*).
  7. **Inventory = mesh-derived dynamic inventory** — a plugin reads the live mesh
     roster (etcd node-tags /mcnf/node-tags/<id> + mackesd peers); roles/scopes
     drive Ansible groups; no static host files.
  8. **Secrets = mde-seal/age** (mesh-native, role/scope-sealed per SEC-003) bridged
     to Ansible via a lookup plugin + a Tofu external data source. NO Ansible Vault
     (single secret system).
  9. **Networking = Nebula overlay** (mesh) + libvirt networks via nmstate/
     NetworkManager (replaces Neutron).
  10. **Removal sequencing = delete OpenStack immediately, build in its place** —
      accept a temporary cloud-ops gap; no permanent compat shim; single cutover.
- Recreate the IaC workspace: rebuild `crates/desktop/mde-shell-egui/src/iac/` as
  the unified cloud-operations surface with modes for Provision (Tofu plan/apply +
  state), Configure (Ansible playbook/role runs), Images (bootc/osbuild), Network,
  Containers (Quadlet), and Status/day-2. Reads provider-neutral state/cloud/*
  mirrors; the `mackes_mesh_types::cloud` facade becomes the live contract (wire
  its real consumers; drop the dormant openstack module import at iac/mod.rs:53).
- Relevant files/components: DELETE `crates/mesh/mackesd/src/workers/openstack/`,
  the OpenStack copy in `cloud_plane.rs`/`console/mod.rs:619`/`front_door.rs:415,462`,
  `state/openstack/*` producers, and OpenStack docs; NEW `infra/tofu/cloud/`
  (libvirt provider, etcd backend, modules), NEW Ansible tree (roles + dynamic
  inventory plugin + site.yml integration), rebuilt `iac/`, the
  `mackes_mesh_types::cloud` facade, `packaging/bootc/`, a new mackesd cloud worker
  (Tofu/Ansible runner + status publisher) registered in WORKER_REGISTRY.
- Dependencies: a farm dev libvirt host to prove list+launch (local; the farm/XCP
  dom0s or a seat). No external cloud creds required (local-first).
- Acceptance criteria: (1) `/audit` grep finds zero product-facing OpenStack/Nova/
  Heat/Keystone/Kolla terminology or code; the `openstack/` worker tree is gone.
  (2) The recreated IaC workspace runs a Tofu apply that provisions a local libvirt
  VM and an Ansible play that configures it, end to end, over mesh networking, with
  no OpenStack present. (3) Tofu state persists in etcd; inventory is mesh-derived;
  secrets resolve via the mde-seal lookup (no Vault). (4) A Podman/Quadlet service
  workload and a bootc image build are driveable from the workspace. (5) Stale
  OpenStack docs archived/bannered.
- Verification method: Tofu+Ansible fixture tests (plan/apply against a libvirt
  fake + a real libvirt host smoke), inventory-plugin unit tests over an etcd
  roster fixture, mde-seal-lookup resolution test, workspace UI fixture tests per
  mode, an `/audit` OpenStack-terminology grep gate, and a live local-libvirt
  provision+configure smoke on a farm/seat host.
- Origin or merged source IDs: QC-1..QC-15, OW-8, E12 supersession notes, operator
  directive 2026-07-19 (remove all OpenStack; OpenTofu provision + Ansible
  configure; recreate IaC workspace for all cloud ops; 10-question Red Hat-standards
  survey).
### WL-ARCH-006 - Workloads cockpit (reenvision the IaC surface: delivery-type x mesh placement)

- Status: Blocked
- Priority: P1
- Complexity: Epic
- Problem: WL-ARCH-001 landed a real-but-coarse OpenTofu+Ansible+libvirt backend + a 6-mode iac/ workspace, but the surface is organized by raw Tofu concepts, cannot place a workload on a specific mesh node, and does not drive the five real delivery types. The operator's 50-question design reenvisions it as "Workloads".
- Required outcome: The iac/ surface (user-facing "Workloads"; seam Surface::InfraCode kept) presents five first-class delivery-type views (Desktop-VM / Service-VM / App-only-VM (VDI app-mode) / Android-VM (Cuttlefish) / Service-Container), each placeable on an explicit mesh node, provisioning + configuring real libvirt workloads end to end over OpenTofu+Ansible. Delete cloud_plane.rs. One-big-cutover.
- Plan: docs/plans/workloads-cockpit.md (locked 50-Q design + 21-unit fan-out + wire contract + per-node-apply reconciliation + ranked risks). Extends WL-ARCH-001 Phase B; supersedes its coarse Phase-C iac/.
- Progress (2026-07-20): **CODE-COMPLETE — all 21 units landed + `cargo build --workspace` green + pushed (origin/master `bae119e6`).** Tier-0 U1a/U2/U3 (wire contract `c7cc9b77` + worker split `bbe859f7` + delivery-type cockpit scaffold `c68e65ec`), Tier-1 U4-U10 backend verbs, Tier-2 U11-U13 (tofu modules + ansible roles), Tier-3 U14-U19 (`74636845` placement picker + provision form; `eeb36d76` 5 delivery-views; `7be0e3ec` configure/inventory + status/metrics; `d13a623f` images/containers), Tier-4 U20+U21 (`bae119e6` — deleted `cloud_plane.rs`/`Plane::Cloud`, Workbench 5→4 planes, de-OpenStacked `unit_aggregator`; `kdc_host/cloud.rs` + `session_broker.rs` were already on the unified path). `Surface::InfraCode`/Workloads reachable + renders. REMAINING (hardware-gated only): live-seat `.15` provision→configure→console→destroy smoke (operator/hardware-gated). The CloudReply rich-payload decode LANDED 2026-07-20 (`72159c31`): `iac/images.rs` decodes the ImageRow roster + console-attach decodes `ConsoleEndpoint` into an honest console section across the delivery views (33 iac tests green); full VDI-paint is a separate subsystem (`main.rs` VdiState). No autonomous work remains — only the hardware-gated live smoke.
- Dependencies: WL-ARCH-001 backend (landed). Live smoke needs a libvirt host (.15) with nested-KVM for the Cuttlefish/Android type (else Android-x86 fallback).
- Acceptance criteria: five delivery-type views each provision+configure a real libvirt workload on a picked node; apply-on-placement-node; armed-token per-request auth; destroy=preview+typed-arm; drift via periodic plan; cloud_plane.rs deleted; zero OpenStack terminology; live provision+configure+destroy smoke on .15.
- Verification method: per-mode egui fixtures; libvirt-fake CloudRunner tests per verb; inventory/mesh.py selftest; /audit OpenStack-terminology grep; live .15 smoke (SSH-verify virsh list + state/cloud JSON + mackesd journal).
- Origin or merged source IDs: WL-ARCH-001 Phase-C successor; operator 50-question Workloads survey 2026-07-20; plan mossy-knitting-sun.md.

## Runtime Reliability

### WL-RUN-003 - Lighthouse full/equal join and push-button add/retire

- Status: Blocked
- Progress (2026-07-20): CODE-COMPLETE per `docs/platform/DRAIN-RECONCILIATION-2026-07-19.md` —
  typed `lighthouse_add`/`lighthouse_retire` (`cli/node_admin.rs:164`/`:205`), etcd voter
  membership (`cli/join.rs` `add_self_as_voter_blocking`, no manual etcdctl), CA
  inheritance, quorum-preserving `drain_gate` (`lighthouse_lifecycle.rs`), and the
  `spawn_lighthouse_onboard` worker + shell flow all landed. BLOCKED-on: the live
  multi-lighthouse fleet add-retire-add cycle drill + DO provisioning creds — operator/
  live-infra gated, not autonomously drainable.
- Priority: P1
- Complexity: Large
- Problem: Lighthouse management still has manual parts around CA custody,
  etcd voter membership, equal/full promotion, and add/retire operations.
- Required outcome: Joining a lighthouse makes it a full/equal participant, and
  add/retire is a single typed operation without manual `etcdctl` or `scp`.
- Scope: Lighthouse role worker, CA custody, etcd voter changes, operator UI,
  audit, and rollback.
- Relevant files/components: lighthouse workers, `docs/ops/do-lighthouses.md`,
  `docs/ops/lighthouse-eagle-migration-recon.md`.
- Dependencies: Live multi-lighthouse fleet.
- Acceptance criteria: A new lighthouse joins with CA/enroll ability and etcd
  voter status; retirement removes it cleanly and preserves quorum.
- Verification method: Live add/retire drill and etcd health proof.
- Origin or merged source IDs: LIGHTHOUSE-TURNKEY, old worklist lines 6223 and
  6224.

## Functional Completeness

### WL-FUNC-011 - Communications collaboration suite full replacement

- Status: Blocked (live-acceptance only — code merged)
- Progress (2026-07-21): CUTOVER LANDED (origin/master a84017f1) — at the AUTONOMOUS CEILING. Phase-1 (56 parity Qs ruled 78408f3b, migration importer 4e0d5df0, retire the dead Kamailio/RTPengine VV stack aad4d511) + Phase-2 shell surface cutover (a84017f1) are merged: Surface::Chat/Voice/Editor retired into one Surface::Communications, Surface::ALL 20->17, mde-voice-egui crate deleted (~3,530 LOC). Surface::Files KEPT (Q26 — not retired). mde-chat/mde-editor-egui/mde-voice-hud kept for their non-surface consumers. Full stack live + §7-audited stub-free: mde-collab-types + mde-collab-core (property-tested convergence) + mackesd CollabWorker (state/collab/* + action/collab/*) + mde-collab-egui CommunicationsSurface mounted in the shell. All 6 parity modes present. Landed farm-green (cargo build --workspace rc=0) with the failing-set proven a subset of base (zero new reds). Parity ledger docs/platform/WL-FUNC-011-parity-ledger.md (519 rows, all 56 open-Qs resolved). REMAINING = Phase-3 live-acceptance gates ONLY, irreducibly operator/hardware-gated: criterion 8 (real WebRTC/SIP call frames — live infra), criterion 9 (DO LLM with a sealed key — operator key), criterion 12 (live visual signoff — cutover shell not yet deployed to a seat; .15 needs an operator-credentialed install). 4 PRE-EXISTING (non-cutover) shell-test reds documented in docs/NEEDS-OPERATOR.md. Phase-3c follow-ups (editor CRDT/three-way-merge/review; call media plane) remain co-edit/hardware-gated.
- Priority: P0
- Complexity: Epic
- Problem: VoIP, Messaging, Alerting, Clipboard, Editor, Files, and Transfers are
  separate surfaces with disconnected identities, histories, workflows, and
  state. Users cannot move naturally between conversation, document editing,
  calls, alerts, shared clipboard content, and file operations inside one
  collaboration context. The existing implementations contain substantial
  working behavior, so a superficial shell around them would leave competing
  stores, navigation, and ownership boundaries rather than deliver one product.
- Required outcome: One complete `Communications` surface replaces all seven
  surfaces without losing existing behavior. Collaboration spaces become the
  organizing object, with messaging, documents, files, transfers, calls, alerts,
  clipboard content, search, and assistive AI sharing one durable, offline-first
  model. The replacement is released only after every surveyed requirement and
  every current-feature parity row is runtime-reachable, tested, and accepted by
  the operator.
- Scope: Full subsystem rewrite; shared collaboration contracts; mesh replication;
  one native egui surface; messaging and threads; document editing and review;
  file management and transfer; alerts; clipboard; voice/video/screen calls; SIP
  interoperability; DigitalOcean-hosted LLM assistance; migration; rollback;
  removal of superseded surfaces, workers, crates, state writers, routes, and
  documentation. Recording, transcription, autonomous AI actions, a competing
  suite-wide omnibox, per-space E2E encryption, partial release, and permanent
  compatibility shims are out of scope.
- Relevant files/components: new `crates/shared/mde-collab-types/`,
  `crates/services/mde-collab-core/`, and
  `crates/desktop/mde-collab-egui/`; `crates/desktop/mde-shell-egui/`;
  collaboration workers under `crates/mesh/mackesd/src/workers/`; existing
  `mde-chat`, `mde-editor-egui`, `mde-files`, `mde-files-egui`, `mde-voice-egui`,
  `mde-voice-hud`, transfer, alert-relay, and clipboard-sync implementations;
  `AI_GOVERNANCE.md` and superseded design notes.
- Dependencies: Coordinate with, but do not duplicate or absorb, WL-ARCH-003 for
  the shared Bus/Persist client, WL-ARCH-004 for worker registration and restart
  policy, WL-FUNC-005 for Start Search indexing, WL-FUNC-006 for shared file
  operation progress, and WL-UX-005 for launcher integration. Final live proof
  requires a sealed DigitalOcean model-access key, multi-node mesh fixtures,
  microphone/camera/display hardware, and SIP test connectivity.

#### Governance, parity, and delivery locks

1. Amend `AI_GOVERNANCE.md` with the newer Communications collaboration lock.
   It supersedes the ICQ-style Chat lock while preserving its signed-message,
   Nebula-transit, Bus-live, and Syncthing-history guarantees. Mark the old Chat,
   notification, clipboard, editor, file-manager, transfer, and voice design
   notes `HISTORICAL / SUPERSEDED`; do not create another active tracker.
2. Before implementation, build a parity ledger inside this epic's evidence trail
   that maps every reachable command, hotkey, menu action, state path, worker,
   CLI verb, migration source, test, and user workflow in the seven replaced
   systems to a Communications replacement or an explicit surveyed retirement.
   No row may be silently dropped.
3. Develop on one integration branch with reviewable commits and internal phase
   gates, but do not release a partial suite, retain a user-facing old/new switch,
   or land dead placeholders on the release branch. The cutover is one immutable
   image release after full parity and operator signoff.
4. Apply `AI_GOVERNANCE.md` section 7 literally: no `todo!()`,
   `unimplemented!()`, stub match arms, mock data presented as functionality,
   unreachable modules, dead controls, or deferred acceptance rows.

#### Public contracts and ownership

1. Add stable identifiers `SpaceId`, `EventId`, `ThreadId`, `DocumentId`,
   `FileRefId`, `TransferId`, and `CallId`. Identifiers are opaque UUID values and
   remain stable across path moves, reconnects, replay, and multi-space linking.
2. Define `SpaceKind` as `Direct`, `Team`, `Incident`, or `Project`, and
   `SpaceRole` as `Owner` or `Member`. A Direct space contains only its named
   participants. Other kinds default to all current mesh members while allowing
   the owner to narrow membership. Owners manage membership and delete spaces;
   members can create and edit content and fully control shared transfer jobs.
3. Define a versioned, Ed25519-signed `CollabEventEnvelope` containing schema
   version, event ID, space ID, actor identity, actor clock, creation timestamp,
   event kind, payload or content-addressed payload reference, and signature.
   Event kinds cover space lifecycle, membership, messages, threads, alerts,
   clipboard items, documents, reviews, file references, transfers, calls, and
   AI suggestion metadata.
4. Define typed `CollabCommand` operations for creating and deleting spaces;
   membership; sending, editing, and deleting messages; thread replies; alert
   acknowledgement and snooze; clipboard publication and attachment; document
   updates and review actions; file linking and deletion; transfer control; call
   lifecycle; and AI suggestion requests. Publish commands under
   `action/collab/*`, retained read models under `state/collab/*`, and live signed
   events under `collab/event/<space>/<actor>`.
5. Define `CollabReadModel` projections for the space directory, Activity,
   conversation/thread timelines, document sessions, file references, transfer
   jobs, alert inbox, clipboard lane, presence, and call state. The egui surface
   reads projections and emits typed commands; it never owns authoritative state
   or calls provider APIs directly.

#### Data flow, replication, and deletion

1. The local collaboration worker validates a command, checks membership and
   time-window policy, signs one or more events, appends them to the actor's
   durable per-space log, projects them transactionally into SQLite, publishes
   the live event over `mde-bus`, and updates retained read models.
2. Syncthing replicates actor logs and content-addressed blobs for offline
   backfill; Bus publication provides the low-latency path. Replayed events are
   idempotent, order-independent, signature-checked, and merged by actor clock
   plus stable event-ID tie-breaking. A disconnected node remains fully usable
   against cached state and converges after reconnection without a fixed center.
3. Store arbitrary MIME payloads and transferred collaboration artifacts by
   SHA-256 in the existing per-user MDE data root. Events carry metadata and blob
   references rather than embedding large payloads in JSON. Verify hash and size
   before projection or materialization.
4. Durable history remains until an authorized explicit deletion. Replicated
   deletion tombstones prevent stale peers from resurrecting data. Purge payloads
   only after every currently known member has acknowledged the tombstone or the
   member has been explicitly removed; retain the minimal tombstone thereafter.
5. Space deletion is direct rather than archive-first and requires confirmation.
   It emits a convergent tombstone for the space and owned collaboration state;
   referenced canonical files are not deleted merely because a space is deleted.

#### Communications surface and navigation

1. Add `Surface::Communications` and remove `Surface::Chat`, `Surface::Voice`,
   `Surface::Editor`, and `Surface::Files` only at final parity. Migrate launcher
   pins, Start Search targets, toast routes, status actions, file-open requests,
   call handoffs, and saved last-surface state to Communications.
2. Use one Office 97 Construct-themed frame built from shared `mde-egui::Style`.
   A persistent left rail lists spaces. Focused mode tabs expose Activity,
   Messages, Documents, Files, Transfers, Alerts, and Clipboard. Direct and
   space-call controls feed one persistent call bar that survives mode and space
   switches.
3. First entry to a space opens Activity; later entries restore that space's last
   focused mode. Activity is an action-oriented chronological feed of meaningful
   messages, edits, comments, file changes, transfers, calls, and alerts, with
   filters but no competing global search box.
4. Desktop and narrow/tablet layouts keep a fixed split between the rail and
   content. Narrow mode compacts the rail to stable icon-sized geometry instead
   of hiding it. Menus, two-row editor toolbars, tabs, call controls, counters,
   and status areas have bounded dimensions and cannot shift or overlap as state
   changes.
5. Connect Communications entities and actions to the existing main Start Search
   index. Panel-local find and filters are allowed; a second suite-wide omnibox
   is not. Notifications use badge counts plus the existing policy-driven toast
   path and route into the exact originating space and object.

#### Messaging, alerting, and clipboard

1. Every space has a Markdown conversation timeline and anchored threads. Enter
   sends by default, drafts persist locally, delivery state is honest, and edits
   and deletion are accepted only for the author's message during the first five
   minutes. A later attempt remains visible as a denied action, not a silent
   no-op.
2. Keep message and thread history until explicit deletion. Preserve sender,
   signature, timestamps, edit history, reply anchor, delivery state, and any
   linked document, file, alert, clipboard item, transfer, or call.
3. Alerting combines source rules with one global inbox projected into relevant
   spaces. Supported workflow actions are acknowledge and snooze; alert severity,
   source, state, and policy determine badges and toasts. Existing emitters keep
   publishing their truthful events and are adapted at the collaboration worker.
4. Clipboard capture is automatic across the mesh and enters one global lane
   before optional attachment to a space or thread. Support arbitrary MIME
   bundles up to 100 MB, previews where safe, copy/materialize actions, source
   attribution, content hashes, and explicit deletion. Larger data must be saved
   or sent through Transfers rather than silently truncated.

#### Ultimate editor and document collaboration

1. Markdown is the canonical document format and the original path remains the
   source of truth. Document mode is the default and provides a one-pane
   Source/Visual toggle, full block editing, ops-oriented templates, optional
   outline, and an Office 97 menu plus two toolbars. Markdown is the only export
   format; print and preview remain available but hidden from the default toolbar.
2. Preserve every existing editor capability in a separate Project mode:
   rope-backed editing, undo/redo, multicursor and column selection, tree-sitter
   highlighting, LSP diagnostics/navigation/rename/format, tabs and split panes,
   project and buffer search, terminal, folding, symbol outline, file finder,
   command palette, and keyboard workflows.
3. Provide full Markdown block semantics, complete table creation and cell/row/
   column editing, hybrid local spell checking plus opt-in cloud grammar review,
   link validation, and image insertion through a file picker. Store document
   images under `<document-stem>.assets/` and write relative Markdown links.
4. Autosave versioned document state, take idle snapshots, and show a timeline
   with rendered word-level diffs and actor attribution. Use an existing Git
   repository when present; otherwise offer, but never silently perform, local
   Git initialization. Do not overwrite unrelated repository history.
5. Use Yrs CRDT updates for live co-editing, shared cursor/selection/viewport
   presence, host/guest access, and follow mode. External or offline writes to
   the canonical path enter a reviewable three-way merge using the last shared
   base, current collaborative state, and disk state; never choose a winner
   silently.
6. Comments, suggestions, message threads, and document annotations use one
   anchored thread model. A portable, versioned review sidecar travels and
   commits with the document. The same `DocumentId` linked into multiple spaces
   shares content and version history while each space keeps separate discussion
   anchors.

#### Files and transfers

1. Preserve complete local and mesh file-manager parity: list/grid/details,
   sorting, hidden files, breadcrumbs, editable paths, history, tabs, dual pane,
   Places/Mesh navigation, selection, drag/drop, previews, archives, search,
   permissions, file operations, and honest degraded states.
2. A space owns references, not a private folder. `FileRefId` maps a stable logical
   identity to owner node, canonical path, filesystem identity where available,
   current content hash, and version history. Suite-driven moves update the path;
   external moves are reconciled by filesystem identity and hash before being
   reported missing.
3. Removing a file from a space deletes only that space's reference. Permanently
   deleting a file is a distinct confirmed action that deletes the canonical file
   and managed replicas, emits a tombstone, and leaves an honest deleted reference
   in historical events. It cannot be presented as undoable.
4. Linking a file to a space starts a resumable, hash-verified transfer to every
   current member. Joining a space automatically backfills all current shared
   files and durable transfer metadata. Every member may pause, resume, cancel,
   retry, reprioritize, and inspect shared jobs through the daemon-owned ledger.
5. Continue reporting all file, archive, browser-download, and collaboration
   transfer progress through the shared bottom-navigation progress model owned by
   WL-FUNC-006; Communications must not create a second progress authority.

#### Calls and media

1. Support direct and space calls with voice, video, and screen sharing. Provide
   complete device selection, mute, camera, screen-source selection, participant
   state, join/leave, retry, and hang-up controls; keep the active call bar visible
   throughout Communications.
2. Use WebRTC P2P for viable direct calls. Use an elected, mesh-reachable LiveKit
   SFU for group calls, failed direct paths, and topology changes. The SFU is an
   ephemeral media relay with no durable collaboration authority and can fail over
   to another capable node.
3. Reuse existing SIP account, DID, provisioning, failover, and G.711 behavior
   behind a LiveKit SIP gateway so PSTN and mesh contacts participate through the
   same call model. Do not maintain a second call history or contact model.
4. Recording and transcription are absent from UI, commands, workers, and storage.
   Audit the selected WebRTC/LiveKit dependency graph and deployment boundary;
   `openssl` and `openssl-sys` remain forbidden in MCNF code, and any necessary
   hosted-media crypto exception requires an explicit governance amendment before
   merge.

#### DigitalOcean LLM integration

1. DigitalOcean Serverless Inference is the only hosted LLM provider. A typed
   `mackesd` adapter calls `https://inference.do-ai.run/v1/responses`, discovers
   permitted models through `/v1/models`, reads a sealed model-access key, and
   exposes provider health and bounded request state through the collaboration
   Bus contract. There is no direct surface HTTP call and no non-DigitalOcean
   fallback.
2. AI is assistive only: rewrite, clarify, summarize, draft, and grammar-review
   operations produce reviewable suggestions. Context is limited to the current
   thread or unread window plus explicitly attached documents/files. Global cloud
   consent is required before the first request and remains revocable.
3. AI never sends messages, edits canonical content, acknowledges alerts, changes
   files, controls transfers, starts calls, or performs other actions. Accepting a
   suggestion is an explicit user edit carrying provider/model attribution in
   document or message history.
4. Timeouts, cancellation, rate limiting, provider unavailability, invalid model
   access, and offline operation surface honest retryable states while every
   non-AI collaboration workflow remains available.

#### Migration, cutover, and removal

1. Add an idempotent importer for signed Chat ring history and rooms, notification
   preferences and alert state, clipboard history, editor open/session/review
   state, file-manager locations and references, transfer ledgers and sync pairs,
   SIP configuration, launcher pins, saved routes, and status/toast destinations.
2. Import into new identifiers using a durable source-to-target map. Re-running
   after interruption must create no duplicate events, blobs, files, transfers,
   or spaces. Preserve canonical files and old state in place so the previous
   OSTree deployment remains a valid rollback target.
3. Before cutover, run read-only parity comparison against old and new projections
   and require zero unexplained differences. On first boot after cutover, perform
   a preflight, backup migration metadata, migrate transactionally, and fail back
   to the previous deployment without partially deleting source state.
4. After all acceptance gates pass, remove the old shell variants, standalone
   routes, duplicate workers, old state writers, retired crates, stale package
   entries, and superseded tests/docs in the same release. Keep only deliberate
   migration readers required to import pre-cutover state; remove them after the
   documented support window rather than retaining general compatibility glue.

- Acceptance criteria:
  1. One Communications entry replaces Chat, Voice, Editor, Files, Transfers,
     Notifications, and Clipboard in the dock, launcher, Start Search, toast,
     status, keyboard, and file-open paths; no competing surface remains.
  2. Direct, Team, Incident, and Project spaces enforce membership and Owner/
     Member behavior, retain their last mode, and remain usable with no peers.
  3. Three nodes creating and editing data during partitions converge after
     reconnect without duplicate events, lost acknowledged work, invalid
     signatures, or resurrection of deleted content.
  4. Markdown messages, threads, five-minute edit/delete, Activity, alert rules,
     acknowledge/snooze, badges/toasts, and 100 MB arbitrary-MIME clipboard
     sharing work with real persisted data and explicit failure states.
  5. Document and Project modes satisfy every editor requirement, live CRDT
     sessions converge, external writes produce a three-way review, comments and
     suggestions remain anchored, and history/Git behavior never destroys user
     data.
  6. The same file can be linked into multiple spaces with one content/version
     identity and separate discussions; reference removal and permanent deletion
     remain distinct; current and newly joined members receive verified files.
  7. Every member can control shared transfers, interrupted transfers resume, and
     all operation progress survives surface and node switches through the shared
     status projection.
  8. Direct P2P and SFU-relayed space calls pass with real advancing audio/video/
     screen frames, SIP ingress and egress pass, call controls remain reachable,
     relay failover is honest, and no recording or transcription artifact exists.
  9. DigitalOcean suggestions use only consented bounded context, are never
     applied automatically, retain provider/model attribution, cancel cleanly,
     and fail without impairing local collaboration.
  10. Office 97 Construct styling, persistent rail, mode tabs, menus, toolbars,
      call bar, dialogs, and dynamic text render without overlap at supported
      desktop and narrow/tablet viewports.
  11. Migration fixtures are repeatable and rollback-safe, the old/new parity
      ledger has no open rows, forbidden dependencies and private D-Bus names are
      absent, and all superseded runtime code is removed after cutover.
  12. The operator completes live visual and workflow signoff with every feature
      present; no incomplete, disabled, placeholder, or deferred behavior remains.
- Verification method: Unit and property tests cover event serialization,
  signatures, ordering, deduplication, permissions, message windows, tombstones,
  blob collection, CRDT convergence, three-way merge, file identity, transfer
  state, call state, AI consent, and migration idempotence. Deterministic two- and
  three-node fixtures cover partition, replay, new-member backfill, member removal,
  duplicate delivery, stale peers, and rollback. Farm gates include focused tests
  for every new crate, affected legacy parity tests, `cargo test --workspace
  --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo fmt --all -- --check`, with the longest job on BigBoy and independent
  jobs parallelized. Live gates cover microphone, camera, screen capture, WebRTC
  P2P, SFU failover, SIP/PSTN, DigitalOcean inference with a sealed key, real file
  backfill, RPM/bootc install and OSTree rollback, plus rendered screenshot and
  canvas-pixel inspection on the production DRM seat at desktop and narrow sizes.
  Final closure requires a reviewed parity ledger and explicit operator visual
  signoff.
- Origin or merged source IDs: `NOTIFY-CHAT`, `EDITOR-1..12`,
  `EDITOR-LSP-1..3`, `EDITOR-COLLAB-1..3`, `EDTB-1..7`, `FILEMGR-*`,
  `TRANSFERS-*`, `E12-11`, `VOIP-GW-*`, Clipboard and alert-relay workstreams,
  operator text-editor survey, and operator 50-question Communications
  collaboration survey completed 2026-07-19.

### WL-FUNC-012 - Maps live-data overlays (zero-cost external feeds)

- Status: Remaining
- Priority: P2
- Complexity: Epic
- Problem: The Maps & Location cockpit's map is a synthetic perspective scene with
  decorative stub overlays (fake cyan weather rect, one orange traffic line in
  `paint_map_scene`) and no lat/lon-to-screen projection; the declared
  traffic/weather/satellite `ProviderContract` seams carry no live data, so the
  vehicle cockpit shows nothing about the road ahead.
- Required outcome: Ten live external overlays land on the map through the proven
  vehicle-worker adapter pattern (poll at feed cadence, publish latest-wins
  `state/overlay/<feed>/<node>` snapshots with `fetched_at`, cockpit folds at 2 Hz,
  gated paint block + Map-tab toggle), on a new vehicle-centered `geo_to_uv`
  local-tangent projection. Catalog (all zero-cost per operator rule 2026-07-22,
  live-verified): NWS alerts, IEM NEXRAD radar tiles, state-511 traffic events,
  NWS gridpoint route forecast, DOT cameras, NIFC+FIRMS wildfire, AirNow AQI,
  adsb.lol ADS-B, GTFS-Realtime transit, USGS quakes. Every feed config carries a
  license-tier tag so a release audit is a grep.
- Plan: docs/design/maps-live-overlays.md (locked 2026-07-22: catalog with verified
  endpoints/cadences/licenses, OVERLAY-0..11 unit fan-out, shared staleness +
  attribution + workstation-side-bandwidth rules, removed-for-cost appendix).
- Relevant files/components: `crates/desktop/mde-maps-location-egui/`
  (`model.rs` MapViewState + folds, `view.rs` paint_map_scene/show_map), new
  overlay workers under `crates/mesh/mackesd/src/workers/`, wire types in
  `crates/mesh/mackes-mesh-types/`, free keys (FIRMS/AirNow/511) via mde-seal.
- Dependencies: `state/vehicle/<node>` GPS fix (Rolling Node MG90 epic) for the
  projection origin and fetch bboxes; outbound internet on the adapter host;
  operator signup for the three free keys (FIRMS, AirNow, 511NY) - keyless feeds
  (NWS, IEM, NIFC, Caltrans, adsb.lol, MBTA/MTA, USGS) are autonomously drainable.
  Coordinates with docs/design/maps-worldclass-plan.md (same surface, 2026-07-22):
  the radar tile unit shares its P2 raster-tile lane under the egui_glow/GLES
  raster-to-egui-texture constraint, and paint hooks serialize behind its
  P0/P1 view.rs/model.rs pipeline per the serialize-same-file rule.
- Acceptance criteria: each overlay paints real live data on a seat with honest
  staleness badges (never stale-as-live); adapters fail soft to idle when
  unconfigured; per-feed toggles + grouped Layers popover + attribution lines;
  Drive HUD defaults to safety layers only; zero paid or non-commercial-licensed
  feeds (the design doc §4 list stays excluded).
- Verification method: FakeProbe-style fixture tests from the captured live
  payloads per adapter; tessellation smoke tests per layer (all-on, NaN fix, tiny
  viewport); live seat deploy with SSH-verified fresh `state/overlay/*` mirrors +
  visual paint check; license-tier grep audit.
- Origin or merged source IDs: operator overlay-planning session 2026-07-22
  (plan help-me-plan-new-hazy-muffin.md; research workflow wf_6731d411-455;
  operator rulings: external-feeds emphasis, vehicle lens, zero-cost only).

### WL-FUNC-013 - Maps & Location world-class + built-for-purpose (offline basemap, geocoding, sparse-data honesty, mode-button)

- Status: Remaining (in progress 2026-07-22)
- Priority: P1
- Complexity: Epic
- Problem: The Maps & Location cockpit reads as FAKE DATA - the simulator fixture
  (a hard-coded Pittsburgh fix + fabricated turn-by-turn to "patrol staging") renders
  as real with no "simulated" marker in Car Mode; unfixed-GPS/absent-signal tiles show
  `0.0000` coords + fabricated accuracy + a `bars(0)==5` full-signal bug; the map is a
  synthetic procedural scene with no real tiles ("do not look real"); there is no
  free-text address entry (only preset destinations + a dead geocoder abstraction);
  and the shell layout-mode button does not visibly switch modes (single-tap only opens
  a menu, misleading MapsLocation glyph, corner-collision with the maps FABs).
- Required outcome: The cockpit reads as a real automotive nav head-unit - sparse-but-real
  MG90 data presented honestly ("Acquiring GPS" / "-", never fabricated zeros); a real
  OFFLINE raster basemap (MBTiles -> egui texture, per the egui_glow/GLES seat constraint
  = raster, NEVER wgpu-vector); free-text address entry + an OFFLINE FTS5 gazetteer
  geocoder; and a labeled single-tap Car<->Desktop mode toggle.
- Plan: docs/design/maps-worldclass-plan.md (2026-07-22 scope report: root-cause analysis,
  GLES raster constraint, P0-P3 units, in-tree reusable seams rusqlite/image/carbon_texture).
- Progress (2026-07-22): Advanced-menu progressive-disclosure nav + floating action cluster
  LANDED (`08086639`); P0 sparse-data honesty pass LANDED (`8b91003c` - has_fix gating,
  bars() fix, idle Drive HUD, un-hideable "SIMULATED" badge, primary source "acquiring" not
  fake-Pittsburgh; 70/0 tests). IN FLIGHT: P0 mode-button (shell toggle), the offline DATA
  build (East-TX gazetteer 16,645 rows built + raster MBTiles building - free OSM on the
  internet-connected control host, bundled to the airgapped seat), and the P1/P2 maps code
  (address entry + FTS5 geocoder + raster basemap renderer).
- Relevant files: `crates/desktop/mde-maps-location-egui/` (car_status.rs, view.rs
  paint_map_scene/show_map, model.rs), `crates/desktop/mde-shell-egui/src/main.rs`
  (layout-mode control), a new `basemap` module + the seat's `client_data_dir/maps/<region>/`
  data pack.
- Dependencies: `state/vehicle/<node>` (Rolling Node MG90) for the live fix; the
  coordinator's outbound internet to BUILD the offline pack (seats airgapped - build here,
  bundle to seat); operator "zero cost" rule (2026-07-22) = free OSM only, no paid map/geocode APIs.
- Coordinates with WL-FUNC-012 (Maps live-data overlays, same surface): the overlays' radar-tile
  unit shares this epic's P2 raster-tile lane; overlay paint hooks serialize behind this
  P0/P1 view.rs/model.rs pipeline per the serialize-same-file rule.
- Verification: farm-green per-crate gates + subset-of-base for the shell tests; live `.15`
  deploy (shell + data pack) + operator visual signoff (Advanced menu, floating buttons,
  honest sparse data, real basemap, working address search, working mode toggle).
- Origin: operator goal "Maps and Location should match world class interfaces, and be
  built for purpose" + operator directives (Advanced menu; floating buttons; "solve all") 2026-07-22.

## User Interface And Experience

## Performance

## Testing And Quality

## Documentation And Maintenance

## Stewardship

How to add, complete, merge, and archive worklist items without regressing into
the pre-2026-07-16 giant-file / parallel-tracker failure. This file is the **only**
active platform worklist; design notes, ops runbooks, review ledgers, and
`docs/NEEDS-OPERATOR.md` are *evidence sources*, not parallel trackers.

### ID scheme

- Every active item is an epic headed `### WL-<FAMILY>-<NNN> - <title>`.
- `FAMILY` is one of the reconciled families: `ARCH`, `BUILD`, `CRIT`, `DOC`,
  `FUNC`, `PERF`, `RUN`, `SEC`, `TEST`, `UX`. Do not invent a new family without an
  operator decision (a new family is a new plane of work, not a convenience).
- `NNN` is a zero-padded, per-family sequence number. A new item takes the next
  free number in its family. **Never reuse or renumber a retired ID** — archived
  IDs stay reserved so old references keep resolving.
- Pre-reconciliation IDs (e.g. `MEDIA-3`, `OW-8`, `FED-RUNTIME`) are **not** valid
  active IDs. Map them to their owning `WL-*` epic via the epic's
  `Origin or merged source IDs` field and the re-key map in
  `docs/NEEDS-OPERATOR.md`.

### Required fields per item

Each `### WL-*` epic carries these fields, in this order:

| Field | Rule |
|---|---|
| `Status` | Exactly one of `Remaining`, `Blocked`, `Needs clarification` (see Status Vocabulary). Closed work is archived, not left with a `Done`/`Completed` status. |
| `Priority` | `P0`..`P3`. |
| `Complexity` | `Small` / `Medium` / `Large` (or `Epic`). |
| `Problem` | The user-visible or correctness gap, not the solution. |
| `Required outcome` | The observable end state that closes the item. |
| `Scope` | The surfaces/systems in and out of scope. |
| `Relevant files/components` | Concrete crates/paths, so the next agent starts from evidence. |
| `Acceptance criteria` | Verifiable conditions; live/hardware proofs named explicitly. |
| `Verification method` | How acceptance is checked (fixture test, live smoke, `@farm:{cargo ...}`). |
| `Origin or merged source IDs` | The pre-reconcile IDs and review handles this epic absorbed — the audit trail. |

`Dependencies` is optional and names a blocking epic or an unmade decision.

### Archive-on-close procedure

- When an item is completed or retired, **move it out of this file** into
  `docs/worklist-archive/` with a one-line disposition (done + evidence, or
  retired + reason). Do not leave closed work in the active file.
- Archive by appending to a dated archive note under `docs/worklist-archive/`
  (see that directory's `README.md`); keep the `WL-*` ID in the archived entry so
  references still resolve.
- A batch reconciliation may temporarily annotate a still-listed epic as
  `Done - <date> ...` in place; that is a reconciliation artifact to be swept into
  the archive at the next stewardship pass, not a new active status value.

### Evidence-citation rule

- Every completion claim cites **file:line**, a live-artifact check, or a wire
  observation — never intent. GUI/runtime claims need farm/live verification or an
  explicit "hardware unavailable" note (per `AGENTS.md`).
- The authoritative evidence ledger for the current epoch is
  `docs/platform/DRAIN-RECONCILIATION-2026-07-19.md`; per-epic `Status:` lines defer
  to it where they disagree.
- Preserve lineage: record absorbed old IDs in `Origin or merged source IDs` rather
  than deleting the history.

### Duplicate-workstream avoidance rule

- One epic per workstream. Before opening a new item, grep existing `WL-*` headings
  **and** their `Origin or merged source IDs` for the topic and any old ID — if it
  is already owned, extend that epic instead of forking a rival.
- Never resurrect a retired tracker (an old `docs/WORKLIST.md`, a design-note
  backlog, or the `NEEDS-OPERATOR` queue) as a second source of truth. Re-key into
  `WL-*` and point the old file at this one.

### Enforcement

- `install-helpers/lint-worklist.sh` guards this file's shape: valid active
  `Status` vocabulary, no retired `- [ ]` checkbox markers, a max line length, no
  credential-shaped tokens, and cargo-only `@farm` build payloads. Run
  `install-helpers/lint-worklist.sh --self-test` to exercise it.
- `install-helpers/lint-doc-supersession.sh` keeps historical design docs honestly
  bannered so a superseded note cannot masquerade as live design (WL-DOC-001).
