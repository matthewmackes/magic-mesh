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
- WL-ARCH-001/WL-ARCH-002/WL-TEST-001: continue Quazar Cloud in parallel with
  substrate work; finish Compute instance verbs/forms first; live smoke creates
  and deletes a nano server instance.
- WL-ARCH-003: begin shared Bus/Persist seam work soon.
- WL-ARCH-004: split worker registration/decomposition into smaller
  worker-family tasks before implementation.
- WL-PERF-001: optimize SPICE dirty rectangles first.
- WL-PERF-002: verify VDI frame wake behavior first.
- WL-UX-001: pass/fail is screenshot/pixel proof on `.15`.
- WL-DOC-001: clean current operator docs first:
  `docs/help/install.md`, `docs/help/node-setup.md`,
  `docs/BUILD-ENVIRONMENT.md`, and `docs/ops/promotion-pipeline.md`.
- WL-DOC-002: merge `docs/NEEDS-OPERATOR.md` fully into this active worklist;
  it should not remain a separate queue.
- WL-DOC-003: require an archive entry for every closed item.
- WL-TEST-002: first harness target is existing live lighthouses; full quorum
  mutation is allowed with rollback.

## Critical Correctness And Data-Loss Risks

### WL-CRIT-001 - Mesh VDI console broker end to end

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Mesh-discovered local KVM/libvirt VM desktops can publish lifecycle
  intent without a dialable console endpoint. The current architecture has
  `desktop_sources` and chooser/session plumbing, but the serving peer still
  needs a real brokered SPICE/RDP/VNC endpoint over the overlay before the shell
  can display pixels for peer-hosted VMs.
- Required outcome: Selecting a peer-hosted VM either opens an interactive
  desktop over Nebula with broker Open to Active state, or the chooser marks the
  lane honestly non-connectable with a reason.
- Scope: Console endpoint resolution, overlay relay or bind, session record
  publication, chooser endpoint resolution, and live transport attach.
- Relevant files/components: `crates/mesh/mackesd/src/workers/desktop_sources.rs`,
  `crates/mesh/mackesd/src/workers/session_broker.rs`,
  `crates/mesh/mackesd/src/workers/vm_lifecycle.rs`,
  `crates/desktop/mde-shell-egui/src/chooser/`,
  `crates/desktop/mde-shell-egui/src/vdi.rs`, `crates/desktop/mde-vdi-*`.
- Dependencies: A live libvirt/Nova host with a guest console and overlay
  reachability.
- Acceptance criteria: Broker resolves a live console port, publishes a dialable
  endpoint in the session/roster record, the shell consumes that endpoint, frames
  and input round-trip, and failed brokering is surfaced without claiming Active.
- Verification method: Unit tests for endpoint resolution and non-connectable
  states, farm build of shell and mackesd, then live seat proof against a real
  guest with frame checksum or video capture evidence.
- Origin or merged source IDs: E12-5, OW-8, QC-13, platform review `vdi-vm-1`,
  old worklist lines 353, 424, 3501.

### WL-CRIT-002 - VDI reconnect and disconnected-state UX

- Status: Remaining
- Priority: P1
- Complexity: Medium
- Problem: A transport drop can leave the desktop frozen on the last frame or
  require manual recovery. Design docs promise reconnectable sessions, but the
  user-visible disconnected state and bounded reconnect loop are not complete.
- Required outcome: Every transport Error or Ended state tears down the dead
  live handle, shows an explicit disconnected overlay, offers Retry and Back to
  Chooser, and attempts bounded auto-reconnect where safe.
- Scope: RDP/VNC/SPICE live handles, broker state transitions, toast/status
  surfacing, and retry/backoff policy.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/vdi.rs`,
  `crates/desktop/mde-shell-egui/src/session.rs`,
  `crates/mesh/mackesd/src/workers/session_broker.rs`,
  `crates/desktop/mde-vdi-rdp/src/connect.rs`.
- Dependencies: WL-CRIT-001 for mesh-hosted console paths; direct endpoint paths
  can be implemented independently.
- Acceptance criteria: Dropping a live transport shows the reason, stops sending
  input into a dead channel, retries with visible state, and returns broker state
  to Active after a successful reconnect.
- Verification method: Targeted unit tests for state transitions and a live
  drop/restore test against at least one transport.
- Origin or merged source IDs: E12-8, platform review `vdi-vm-4` and
  `shell-ux-1`, old worklist line 366.

### WL-CRIT-004 - Control-plane DR backup and guided rebirth

- Status: Blocked
- Priority: P0
- Complexity: Large
- Problem: Backup/restore code exists for state and secrets, but the remaining
  DR acceptance depends on off-fleet CA/secret export, an operator-controlled
  target, and a guided restore that rebirths the control plane and re-elects a
  leader without unsafe secret handling.
- Required outcome: A documented and tested DR path backs up Tofu state, Nebula
  CA material, and secret store data to an off-fleet encrypted target, then
  restores a fresh control plane with a verified leader and usable enroll path.
- Scope: DR scripts, scheduler/RPC/button, CA-holder workflow, off-fleet target,
  restore runbook, and safety classification.
- Relevant files/components: `automation/dr/`, `docs/help/mesh-recovery.md`,
  `crates/mesh/mackesd/src/ca/`, `crates/mesh/mackesd/src/workers/`.
- Dependencies: Operator-run off-fleet export target and CA-holder access.
- Acceptance criteria: Backup bundle is produced without plaintext in logs or
  argv, restore verifies the bundle, a fresh node can enroll after restore, and
  the leader election is healthy.
- Verification method: Operator-run DR drill with logs redacted, plus local
  dry-run tests that never exfiltrate live secrets.
- Origin or merged source IDs: DR #4, DATACENTER-23, DAR-42, old worklist lines
  615 and 2507.

### WL-CRIT-005 - Substrate-v2 fleet cutover and LizardFS wedge removal

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: Incident notes show live FUSE/LizardFS wedge risk remains until the
  fleet is cut over to etcd plus Syncthing and the old single-master mount
  dependency is retired.
- Required outcome: The live fleet runs the substrate-v2 path with no required
  LizardFS/QNM mount for control-plane correctness, and reboot/failover does not
  reintroduce a wedged FUSE dependency.
- Scope: Fleet cutover, live lighthouses, QNM retirement, Syncthing/etcd
  verification, incident cleanup, and runbook updates.
- Relevant files/components: `automation/substrate/`, `docs/ops/substrate-v2-cutover-runbook.md`,
  `docs/ops/lighthouse-eagle-migration-recon.md`, mackesd substrate workers.
- Dependencies: Live maintenance window and operator authority on the deployed
  lighthouses.
- Acceptance criteria: Cutover completes on live nodes, no critical worker needs
  the retired FUSE mount, reboot recovery passes, and old wedge incidents cannot
  recur through the same dependency.
- Verification method: Operator-run cutover log, reboot/recovery gate, and
  post-cutover health snapshots.
- Origin or merged source IDs: OPROG-1, OPROG-2, LH-JOIN-QNM-1,
  INCIDENT-WEDGE-2, old worklist lines 2210, 2227, 2230, 2251.

## Security

### WL-SEC-001 - Fresh-node enrollment bootstrap and final join path

- Status: Blocked
- Priority: P1
- Complexity: Medium
- Problem: Fresh-node bootstrap has staged fixes, but final live enrollment can
  still fail when a token carries an overlay endpoint instead of the public
  enroll endpoint needed before Nebula is up.
- Required outcome: Every supported invite/enroll token form carries or resolves
  the correct public enroll endpoint and fingerprint for non-overlay bootstrap,
  then joins into the overlay cleanly.
- Scope: Invite token shape, join/enroll CLI, onboarding wizard, public enroll
  endpoint selection, and CSR signing.
- Relevant files/components: `crates/mesh/mackesd/src/onboard/`,
  `crates/mesh/mackesd/src/bin/mackesd.rs`, onboarding UI, `docs/NEEDS-OPERATOR.md`.
- Dependencies: Live lighthouse endpoint and fresh test node.
- Acceptance criteria: A fresh Fedora node joins from the advertised token with
  no manual endpoint correction, obtains an overlay IP, and survives reboot.
- Verification method: Live fresh-node join drill plus parser/unit tests for
  legacy and v3 token forms.
- Origin or merged source IDs: OW-4 residual, DAR-19, LH-JOIN-QNM-1,
  `docs/NEEDS-OPERATOR.md`.

### WL-SEC-002 - Federation runtime enforcement and two-mesh accept flow

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Federation docs and operator queue still describe accepted designs
  without a complete runtime consumer and GUI flow for true two-mesh acceptance.
- Required outcome: Federation policy is enforced by runtime code, not only
  represented in configuration, and the GUI exposes safe accept/refuse actions.
- Scope: Federation runtime consumer, cross-mesh identity checks, GUI flow, audit
  events, and refusal paths.
- Relevant files/components: federation workers, shell federation/provisioning
  surfaces, `docs/NEEDS-OPERATOR.md`.
- Dependencies: A second test mesh or deterministic local federation harness.
- Acceptance criteria: A foreign mesh cannot gain access without explicit accept,
  accepted links are audited, and stale/foreign offers are rejected.
- Verification method: Integration test with two mesh identities plus GUI action
  tests.
- Origin or merged source IDs: FED-RUNTIME, FED-XMESH, FED-GUI,
  `docs/NEEDS-OPERATOR.md`.

### WL-SEC-003 - Secret-store distribution and scoped decryption roots

- Status: Blocked
- Priority: P1
- Complexity: Large
- Problem: Secret-store code exists, but several live acceptances still require
  multi-node distribution, credential rotation proof, and a clear answer for
  scope separation so one identity is not an unnecessary fleet-wide decryption
  root.
- Required outcome: Secrets needed by datacenter, media, router, DR, and control
  VM paths are replicated only to authorized recipients and rotate without
  plaintext exposure.
- Scope: Age recipient sets, etcd-backed secret store, role-scoped recipients,
  rotation flows, and live multi-node distribution.
- Relevant files/components: `automation/secrets/`, `crates/mesh/mde-seal/`,
  `crates/mesh/mackesd/src/workers/openstack/secrets.rs`,
  `crates/mesh/mackesd/src/workers/`.
- Dependencies: Live second node with recipient registration and operator-owned
  credentials for at least one provider.
- Acceptance criteria: Two authorized nodes decrypt the same secret, an
  unauthorized node cannot, rotation redistributes to the right set, and no
  secret appears in argv/logs/tfvars.
- Verification method: Multi-node live secret test plus offline fixture tests for
  recipient selection and rotation.
- Origin or merged source IDs: DATACENTER-3, DS-8, XCP-7, security review
  `security-6`, MEDIA-2 residual, old worklist lines 2156, 2157, 2363.

### WL-SEC-004 - Phone remote-input authorization and visible indicator

- Status: Remaining
- Priority: P1
- Complexity: Medium
- Problem: Phone remote input and media control are powerful enough to inject
  system input. The security review requires per-action authorization and a
  visible on-seat indicator so local users can see and control remote input.
- Required outcome: Phone-as-touchpad/media-control requires explicit trust,
  shows an on-screen active indicator, and refuses forged/local Bus injection.
- Scope: KDC host worker, remote input path, indicator UI, allowlist/confirmation,
  and audit events.
- Relevant files/components: `crates/mesh/mackesd/src/workers/kdc_host.rs`,
  `crates/desktop/mde-shell-egui/src/phones_hub.rs`, input injection path.
- Dependencies: Paired phone or loopback KDC protocol harness.
- Acceptance criteria: Untrusted input is rejected, trusted input displays an
  indicator while active, media keys work after consent, and revocation stops
  input immediately.
- Verification method: KDC protocol tests plus live phone or loopback remote-input
  smoke.
- Origin or merged source IDs: KDC-MESH-6, KDC-MESH-4, platform review
  `security-7`, old worklist lines 3332 and 3334.

## Build, Installation, And Deployment

### WL-BUILD-001 - Immutable bootc, ISO, and RPM release gate

- Status: Blocked
- Priority: P1
- Complexity: Large
- Problem: Bootc image, ISO, Fedora RPM, and headless Workstation paths are
  partly implemented, but release acceptance requires live boot, signing, and
  registry/channel steps.
- Required outcome: A deployable Workstation/headless Workstation image boots,
  enrolls, starts the shell or headless services, and matches the published RPM
  payload.
- Scope: Bootc Containerfile, ISO/kickstart, RPM payload, signing, registry
  publish, role-gated units, and boot verification.
- Relevant files/components: `packaging/bootc/`, `packaging/kickstart/`,
  `install-helpers/build-rpm-fedora43.sh`, `automation/promotion/`.
- Dependencies: Live boot hardware/VM, signing material, and release authority.
- Acceptance criteria: Fresh install boots, role selection works, mackesd and
  shell/headless services start, rollback path is documented, and payload gates
  pass.
- Verification method: Farm RPM lane, boot smoke, promotion L1/L2 gates, and
  live hardware confirmation.
- Origin or merged source IDs: E12-13, OW-12, BOOT-REC-4, old worklist lines 384,
  429, 1430.

### WL-BUILD-002 - Farm shared cache and fresh-farm bootstrap

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: The farm has demand parsing and many successful lanes, but shared
  sccache/control-VM bootstrap and fresh-farm one-shot proof remain live-gated.
- Required outcome: A fresh farm node can bootstrap, join, build with shared
  cache hits, and return to a clean slot state without manual warmup.
- Scope: Golden build image, sccache backend, farm bootstrap script, slot cleanup,
  snapshot/revert, and documentation.
- Relevant files/components: `install-helpers/farm.sh`,
  `install-helpers/xcp-build.sh`, `install-helpers/farm-reconciler.sh`,
  `install-helpers/farm-sccache-proof.sh`, `automation/farm/`,
  `docs/BUILD-ENVIRONMENT.md`.
- Dependencies: Build farm control VM and live farm nodes.
- Current evidence: A 2026-07-17 shared-cache proof pass added
  `install-helpers/farm-sccache-proof.sh` and corrected
  `docs/BUILD-ENVIRONMENT.md` so it no longer claims shared sccache is live
  before proof. The live farm check reached `.50`, `.90`, `.130`, and `.170` and
  all four nodes reported no `sccache` binary and no `~/.sccache.env`, so the
  item remains open and accurately live-backed.
- Acceptance criteria: Node A build produces cache hits on node B, fresh-farm
  bootstrap completes, and slots drain without abandoned artifacts.
- Verification method: Farm lane with explicit `MCNF_BUILD_HOST` and
  `MCNF_BUILD_SLOT`, `install-helpers/farm-sccache-proof.sh status`, and
  sccache stats.
- Origin or merged source IDs: FARM-AUTO-PROD, DAR-34, DAR-35, DAR-36,
  old worklist lines 2265, 2277, 2278, 3011, 3019, 3027.

### WL-BUILD-003 - Promotion rollback, version matrix, and secret-scan gates

- Status: Remaining
- Priority: P1
- Complexity: Medium
- Problem: Promotion is stronger than earlier reviews, but rollback/downgrade,
  Fedora version compatibility, and automated secret scanning need to be explicit
  gates so release safety does not rely on memory.
- Required outcome: Promotion supports forward and rollback flows, documents the
  Fedora target matrix, and rejects credential-shaped content in worklists,
  docs, scripts, and generated artifacts.
- Scope: Promotion pipeline, release notes, CI/farm gates, gitleaks or local
  deny-list scan, and rollback runbook.
- Relevant files/components: `automation/promotion/`, `docs/ops/promotion-pipeline.md`,
  `.github/workflows/`, `install-helpers/verify-*`.
- Dependencies: Valid release candidate and repo secret-scan policy.
- Current evidence: The 2026-07-17 BigBoy Fedora 44 container RPM cut initially
  failed with `cannot update the lock file /src/Cargo.lock because --locked was
  passed`; commit `955cacf9` reconciled the missing `mde-shell-egui` -> `tokio`
  lockfile edge, and the same F44 lane then produced base and Browser RPMs under
  the size guard. If this recurs, preserve the release `--locked` contract and
  reconcile `Cargo.lock` rather than disabling `MDE_RPM_LOCKED`.
- Acceptance criteria: A candidate can be promoted and rolled back in test,
  Fedora compatibility is documented, and a planted credential fails the gate.
- Verification method: Non-production promotion drill, secret-scan fixture, and
  docs grep for version claims.
- Origin or merged source IDs: platform review `build-deploy-1`,
  `build-deploy-4`, `build-deploy-5`, old compliance findings.

## Core Architecture

### WL-ARCH-001 - Quazar Cloud hard cutover to Nova/libvirt/QEMU-KVM

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Governance says cloud-hypervisor is retired, but historical docs and
  worklist text still carry old-stack assumptions while Quazar Cloud has several
  live acceptance gates open.
- Required outcome: Cutover nodes run the Nova/libvirt/QEMU-KVM plus OVN stack,
  old stack code is absent from runtime paths, and stale cloud-hypervisor
  directions are either archived or bannered as historical.
- Scope: Kolla/OpenStack services, image pipeline, networking, old-stack deletion,
  docs cleanup, and live cloud status.
- Relevant files/components: `crates/mesh/mackesd/src/workers/openstack/`,
  `docs/design/quasar-cloud.md`, `docs/ops/quasar-cloud-runbook.md`,
  `packaging/bootc/`, cloud UI.
- Dependencies: Farm dev cloud/test bed and live cloud credentials.
- Acceptance criteria: API catalog is healthy, instances launch over mesh
  networking, old-stack binaries/modules are not used, and docs point to the
  current architecture.
- Verification method: Farm dev cloud lane, `/audit` old-stack grep, live cloud
  smoke.
- Origin or merged source IDs: QC-1 through QC-15, OW-8, E12 supersession notes,
  old worklist lines 3457-3567.

### WL-ARCH-002 - Cloud resource verbs, forms, and typed arming

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Cloud catalog and compute lifecycle paths exist, but generic
  create/update/delete forms and verbs for all resource kinds remain partial or
  omitted.
- Required outcome: Resource operations are catalog-driven, typed, armed before
  destructive mutation, audited, and backed by real Bus/OpenStack calls.
- Scope: Cloud UI forms, action verbs, typed arming, audit log, Heat/Octavia
  integration, and linked cross-service views.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/iac/`,
  `crates/mesh/mackesd/src/workers/openstack/verbs.rs`,
  `crates/mesh/mackesd/src/workers/openstack/client/`.
- Dependencies: WL-ARCH-001 and an OpenStack test project.
- Current evidence: A 2026-07-17 Fleet/Data Center copy pass kept unsupported
  container lifecycle verbs absent from the container roster while replacing visible
  implementation/backlog wording with an operator-facing inventory-only note; farm
  `.50` fmt and BigBoy `.130` focused
  `datacenter_container_inventory_note_is_operator_facing` passed.
- Acceptance criteria: Compute, network, volume, image, and orchestration rows can
  list/show and run implemented mutations; unsupported verbs are absent, not dead
  buttons.
- Verification method: Unit tests with contract fixtures plus live create/delete
  smoke in a throwaway project.
- Origin or merged source IDs: QC-13, QC-16, QC-18, IAC partial rows, old
  worklist lines 4446, 4447, 4473.

### WL-ARCH-003 - Shared Bus/Persist client seam and wire-contract fixtures

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: Many shell surfaces still open their own store/Bus connections or
  mirror wire shapes, increasing poll cost and drift risk.
- Required outcome: A shared shell Bus/Persist client seam owns connection reuse,
  latest-value reads, and fixture-backed wire contracts across desktop/mesh
  boundaries.
- Scope: Shell state model, Bus client, topic polling, contract fixtures,
  migration of high-traffic surfaces, and tests.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/`,
  `crates/platform/mde-bus/`, `crates/shared/mackes-mesh-types/`.
- Dependencies: Agreement on seam API and staged migration to avoid a risky
  all-at-once refactor.
- Acceptance criteria: High-frequency surfaces no longer open SQLite per tick,
  shared fixtures cover mirrored wire types, and no behavior changes in UI flows.
- Verification method: Focused shell tests, grep for reduced `Persist::open`
  sites, and performance trace of poll-heavy surfaces.
- Origin or merged source IDs: platform review `arch-11`, `arch-6`,
  `test-obs-8`, open ledger `arch-11`.

### WL-ARCH-004 - Mackesd worker registration, decomposition, and restart policy

- Status: Remaining
- Priority: P2
- Complexity: Epic
- Problem: `mackesd` worker wiring remains a major maintenance hazard despite
  later cleanup. Worker registration, restart policy, and family boundaries need
  a declarative shape.
- Required outcome: Worker families register through a single table carrying
  name, role/capability, constructor, and restart policy, with large families
  split behind stable traits where useful.
- Scope: `run_serve`, worker registry, role census, restart policy, and
  family-level modules/crates.
- Relevant files/components: `crates/mesh/mackesd/src/bin/mackesd.rs`,
  `crates/mesh/mackesd/src/workers/`, `crates/mesh/mde-worker-core/`.
- Dependencies: No active worker refactor in flight; staged PRs to keep review
  possible.
- Acceptance criteria: New workers are added in one registry, supervisor restart
  policy is explicit, and tests prove role-gated workers still spawn correctly.
- Verification method: Mackesd focused tests, role-provision tests, and compile
  time/build impact comparison.
- Origin or merged source IDs: platform review `arch-1`, `arch-5`, `mackesd-07`,
  open ledger `arch-7` residual.

### WL-ARCH-005 - Browser worker crypto seam and mde-seal emitter completion

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: Browser worker extraction is mostly done, but passkey/credential
  crypto still needs a shared seal/crypto seam, and `mde-seal` carries emitter
  placeholders that should become a real generated-contract path or be removed.
- Required outcome: Browser passkey/secret operations use shared, tested crypto
  primitives and `mde-seal` has no dormant placeholder emitter paths.
- Scope: Shared crypto crate API, browser passkey workers, seal emitter, tests,
  and docs.
- Relevant files/components: `crates/mesh/mde-seal/src/lib.rs`,
  `crates/mesh/mde-browser-workers/`, `crates/mesh/mackesd/src/workers/browser_*`.
- Dependencies: Crypto API review.
- Acceptance criteria: No placeholder returns remain in production paths; browser
  passkey workers use the shared seam; old duplicate crypto helpers are gone or
  archived.
- Verification method: Unit tests for seal/passkey flows, grep for placeholder
  emitter paths, and cargo test for browser worker crates.
- Origin or merged source IDs: open ledger `arch-7`, TODO scan of `mde-seal`.

## Runtime Reliability

### WL-RUN-001 - Auto-repair must either repair or say observe-only

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: The reconciler can queue repair intent while the actual take-action
  layer is gated, creating a say/do gap for self-healing claims.
- Required outcome: Either implement the take-action repair executor over the
  current substrate, or make observe-only status explicit in health/UI/audit text
  and track the executor separately.
- Scope: Reconcile worker, audit wording, health output, UI status, and repair
  executor.
- Relevant files/components: reconcile worker, openstack reconcile paths,
  health/status UI.
- Dependencies: Connectivity substrate decisions for safe repair actions.
- Acceptance criteria: A detected drift either changes state through a tested
  executor or records a clearly non-repairing observation; no row says queued as
  if action occurred.
- Verification method: Unit test with injected drift and audit assertions; live
  dry-run on non-destructive drift.
- Origin or merged source IDs: platform review `mackesd-03`.

### WL-RUN-002 - Failure-rate metrics and process-wide counters

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: Metrics export includes important gauges, but rate counters for worker
  restarts, reconcile failures, drift events, and Bus publish errors are still
  incomplete.
- Required outcome: A process-wide counter registry is incremented by producers
  and rendered by the Prometheus exporter with stable metric names.
- Scope: Metrics registry, worker supervisor, reconcile paths, bus publish error
  sites, exporter, and alert examples.
- Relevant files/components: `crates/mesh/mackesd/src/metrics.rs`,
  `crates/mesh/mackesd/src/workers/metrics_exporter.rs`,
  `crates/mesh/mackesd/src/workers/mod.rs`.
- Dependencies: Metric naming review.
- Acceptance criteria: Counters increment in production paths, exporter renders
  them, and tests cover at least worker restart plus reconcile failure.
- Verification method: Unit tests plus a local exporter scrape.
- Origin or merged source IDs: platform review `test-obs-9`.

### WL-RUN-003 - Lighthouse full/equal join and push-button add/retire

- Status: Remaining
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

### WL-RUN-004 - Media lighthouse production service, failover, and upload path

- Status: Blocked
- Priority: P1
- Complexity: Large
- Problem: Media lighthouse infrastructure has many completed slices, but
  production service account handling, upload/rescan, fresh-node browse, and
  failover verification remain gated by live media nodes and operator assets.
- Required outcome: At least two Lighthouse_Media nodes serve the same library,
  `music.mesh` fails over, a non-admin shared account is provisioned, uploads
  trigger rescans, and a fresh Workstation browses/plays automatically.
- Scope: Media role, Navidrome worker, DO Spaces mount, shared account, registry,
  upload/rescan path, DNS/failover, and mde-music autoconfig.
- Relevant files/components: media workers, `automation/media/`,
  `crates/desktop/mde-music-egui/`, `docs/ops/media-ingestion.md`.
- Dependencies: Live DO Spaces bucket/keys and live Lighthouse_Media nodes.
- Acceptance criteria: Kill-one streaming survives within recorded retry window,
  uploads appear after rescan, and Workstations receive non-admin credentials.
- Verification method: Live media drill with two nodes and mde-music client proof.
- Origin or merged source IDs: MEDIA-1 through MEDIA-10, MEDIA-9, OW-11,
  old worklist lines 2162-2207.

### WL-RUN-005 - Device Manager multi-source inventory and fault notifications

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: Device Manager needs source coverage and eventing beyond local PC
  inventory: Cloud/Nova instances, paired phones, LAN hosts, routers, and fault
  transitions should render accurately and notify without spam.
- Required outcome: Each host type contributes only applicable categories, and a
  transition into problem state emits a debounced notification to Chat/phone.
- Scope: Source adapters, host rail, device tree rendering, fault detector,
  notification routing, and tests.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/device_manager/`,
  Nova registry, KDC, LAN probe, router registry, chat alert paths.
- Dependencies: Representative source data for each host type.
- Acceptance criteria: Tests map each source type to the right categories; a
  simulated fault fires once; flapping does not spam.
- Verification method: Unit tests with fixtures plus live test-bed render.
- Origin or merged source IDs: Device Manager open bullets, old worklist lines
  4369, 4370, 4386-4395.

### WL-RUN-006 - Router discovery and firewall commit-confirm control

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: Router discovery/control has a partial design and some YAGNI-scoped
  state work, but per-node discovery, credential state, panel rendering, and
  firewall commit-confirm need reconciliation with current shell architecture.
- Required outcome: Routers are discovered by MAC/IP/vendor, credentials are
  sealed by the operator, managed routers show live status, and firewall edits
  use typed confirm plus auto-rollback.
- Scope: Router registry worker, secret keying, panel UI, EdgeOS/VyOS adapters,
  firewall edit verb, audit.
- Relevant files/components: router worker/design, datacenter/device surfaces,
  `docs/design/router-control.md`.
- Dependencies: Live router credentials and test appliance.
- Acceptance criteria: Uncredentialed routers show managed state honestly;
  credential sealing flips state; a firewall edit auto-reverts if unconfirmed.
- Verification method: Unit tests with fixture banners plus live router smoke.
- Origin or merged source IDs: ROUTER-1 through ROUTER-7, old worklist lines
  2669-2711.

## Functional Completeness

### WL-FUNC-001 - Browser protected media and hardware media path

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: CEF base operation is strong, but protected media, PiP, background
  audio, media keys, GPU/HW decode, and long-running playback are not all proven.
- Required outcome: DRM/protected-media sites work when Widevine is fetched by
  the user, non-DRM browsing still works without it, and media playback remains
  smooth on the live seat.
- Scope: Widevine fetch/install gate, protected-media permissions, media session
  control, PiP/background audio, GPU decode, and live smoke.
- Relevant files/components: `crates/desktop/mde-web-cef/`,
  `crates/desktop/mde-shell-egui/src/web/`, browser runtime installer.
- Dependencies: Widevine-capable CEF runtime and live test account/content where
  legally usable.
- Acceptance criteria: A protected-media smoke passes or is blocked with a named
  external requirement; normal browser works without CDM; media keys and PiP
  route through browser chrome.
- Verification method: Farm CEF tests plus live DRM/Spotify/Netflix-equivalent
  operator smoke.
- Origin or merged source IDs: BROWSER-DD-4, BROWSER-DD-9, old worklist lines
  4111 and 4184.

### WL-FUNC-002 - Browser passkeys, hardware keys, and phone authenticator

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: Browser passkey consent and software shapes have landed, but hardware
  CTAP2 keys, PIN/biometric verification, phone-as-authenticator, attestation, and
  real-site passwordless login remain unproven.
- Required outcome: Browser WebAuthn supports approved credential flows with
  honest User Presence/User Verification semantics and live third-party proof.
- Scope: CTAP2 hardware path, platform authenticator, KDC phone authenticator,
  attestation policy, UI prompts, and site compatibility.
- Relevant files/components: `crates/mesh/mde-browser-workers/`,
  `crates/desktop/mde-shell-egui/src/web/`, `crates/desktop/mde-web-cef/`,
  KDC components.
- Dependencies: Hardware key and test identity provider.
- Acceptance criteria: Hardware key login works, phone authenticator works or is
  explicitly gated, shell consent remains required, and UV is never asserted
  without real verification.
- Verification method: Browser worker tests plus live WebAuthn smoke against a
  controlled relying party.
- Origin or merged source IDs: BROWSER-DD-6, passkey review residuals, old
  worklist line 4123.

### WL-FUNC-003 - Browser mesh sync, follow-me tabs, and bookmark integration

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: The system bookmark manager exists, but Browser still needs complete
  mesh sync for tabs/session, settings, speed dial, downloads list, bookmarks,
  follow-me tabs, and send-tab flows.
- Required outcome: Browser state follows the user over the Nebula/Syncthing
  substrate without an external account, while using the system bookmark manager
  as the source of bookmark truth.
- Scope: Browser state model, mde-bookmarks integration, session sync, send-tab,
  downloads list, conflict handling, and settings sync.
- Relevant files/components: `crates/services/mde-bookmarks/`,
  `crates/desktop/mde-bookmarks-egui/`,
  `crates/desktop/mde-shell-egui/src/web/`, sync workers.
- Dependencies: Mesh substrate and bookmark service availability.
- Current evidence: The 2026-07-17 Browser bookmark truthfulness pass confirmed
  the Browser mirrors `state/bookmarks/collection` into bar links, bookmarked URL
  membership, and omnibox bookmark candidates, and tightened the page-action
  star/menu so pages already present in the system bookmark manager show a
  disabled `Bookmarked` row instead of offering duplicate `Add bookmark`; farm
  `.50` fmt and BigBoy `.130` focused page-actions coverage passed.
- Acceptance criteria: A tab/bookmark/settings change on node A appears on node B,
  conflicts converge, and Browser does not maintain a competing bookmark store.
- Verification method: Multi-node sync test or deterministic two-store fixture,
  plus Browser UI regression.
- Origin or merged source IDs: BROWSER-DD-7, user decision "Use system bookmark
  manager", old worklist line 4139.

### WL-FUNC-004 - Browser power tools, downloads, PDF/print, capture, and protocol handling

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: Browser has many first-party tools, but the daily-driver tail still
  includes Power mode, DevTools/view-source/UA/device APIs, downloader/scraper,
  full PDF/print/save-as-PDF, capture, translation/cache, notifications, and
  protocol handlers.
- Required outcome: Each tool is either implemented through the Browser command
  model or intentionally absent with no dead menu item.
- Scope: Browser command model, download manager, PDF/print, capture, DevTools,
  protocol handlers, offline/cache/translation, and notifications.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/web/menubar.rs`,
  `crates/desktop/mde-shell-egui/src/web/chrome_ui/`, capture/printing modules,
  transfer service.
- Dependencies: CUPS/printing environment and transfer service.
- Current evidence: The 2026-07-17 menu truthfulness pass tightened the Browser
  command model so Browser-owned internal pages no longer advertise helper/page
  tools, stale saved-PDF paths no longer enable `Open Last PDF`, and the no-page
  menu gate still leaves only genuine chrome/bookmark-manager controls active.
  Farm evidence: `.50` fmt, BigBoy `.130` internal-page menu test, `.90`
  stale-PDF menu test, and `.170` no-page menu test passed. Live `.15` still
  has the installed split Browser RPMs and active shell service, but package
  replacement/runtime smoke remains blocked by non-interactive sudo.
  A later 2026-07-17 Browser Options pass replaced the generic disabled-row
  tooltip with command-specific gate explanations for typed-address, history,
  live web-page tools, painted-frame captures, saved-PDF viewer, CEF DevTools,
  loaded-URL share/download actions, first-party-site permission actions, and
  data-clear actions; farm `.50` fmt and `.130` focused
  `browser_options_disabled_rows_explain_their_command_gate` passed.
  A later 2026-07-17 Browser downloads drawer pass removed internal
  `browser_download`/ledger wording from the drawer header, replaced it with a
  user-facing live status summary derived from active/total Browser transfer
  counts, and kept the empty worker state honest without exposing implementation
  terms; farm `.50` fmt and `.130` focused
  `browser_download_drawer_header_uses_user_facing_status` passed.
  A later 2026-07-17 Browser artifact identity pass centralized the Browser
  product label, kept the new-tab dashboard on the same label, and changed
  capture/PDF folders, MHTML/offline-copy subjects, and generated CUPS job
  titles from superseded `Magic Mesh Browser` wording to `Quazar Browser`; farm
  `.50` fmt plus BigBoy focused artifact, dashboard, and CUPS title tests passed.
  A later 2026-07-17 Browser menu copy pass removed internal follow-up/v1/stub
  language from visible Power/Privacy captions while keeping the command gates
  intact; farm `.50` fmt plus `.90` focused Privacy menu coverage passed, and
  BigBoy `.130` focused Power menu coverage passed after session recovery.
  A later 2026-07-17 Browser scrape-export copy pass removed internal follow-up
  hook wording from generated Markdown artifacts, kept the bounded crawl status
  honest, and covered both no-DOM and DOM-backed scrape exports; farm `.50` fmt
  and BigBoy `.130` focused `scrape_export` coverage passed.
  A later 2026-07-17 Power-menu polish pass replaced implementation/backlog
  wording in visible media-manifest and scrape-export captions with user-facing
  behavior language and extended the Power-menu copy guard; farm `.50` fmt and
  BigBoy `.130` focused `power_mode_adds_power_menu_and_status_chip` passed.
  A later 2026-07-17 Browser Options copy pass replaced visible runtime,
  helper-backed, and internal-tab wording with user-facing Controls, Engines,
  open-tab, and live-web-page labels; BigBoy `.130` focused `browser_options`
  coverage passed.
  A later 2026-07-17 Browser downloads unavailable-state copy pass replaced the
  remaining visible Transfers worker wording with Browser-facing downloads
  unavailable copy and extended drawer guards against worker/helper/ledger terms;
  farm `.50` fmt and BigBoy `.130` focused drawer header/muted-note tests passed.
  A later 2026-07-17 Browser engine copy pass replaced visible `CEF / Chromium`
  and `Chromium/CEF runtime` wording in engine menu rows, Options, hover cards,
  DevTools gates, and runtime notices with user-facing Chromium labels while
  preserving CEF implementation markers; farm `.50` fmt, `.90` menu/hover tests,
  `.170` chrome-ui label tests, and BigBoy `.130` live-helper gate test passed.
  A later 2026-07-17 Browser update-status copy pass replaced raw updater state,
  runtime paths, CEF wording, and manifest errors in the engine update drawer and
  launch-block notice with Chromium-facing labels and sanitized verification
  details; farm `.50` fmt, `.90` focused drawer render coverage, `.170`
  status-chip coverage, and BigBoy `.130` live-helper gate coverage passed.
  A later 2026-07-17 Browser export/notice copy pass removed helper, handoff,
  and CEF viewer/tab wording from scrape Markdown artifacts, PDF viewer notices,
  DevTools gates, and malformed passkey notices while preserving the underlying
  CEF implementation paths; farm `.50` fmt, BigBoy `.130` scrape-export coverage,
  `.90` saved-PDF viewer coverage, and `.170` DevTools/passkey notice coverage
  passed.
  A later 2026-07-17 Browser speech/passkey copy pass moved read-aloud, voice
  input, and passkey approval chrome off raw TTS/STT/CEF/runtime wording while
  preserving worker payloads; farm `.50` fmt plus speech-status parser/display
  coverage, BigBoy `.130` drawer/prompt paint coverage, `.90` menubar
  chip/read-aloud notice coverage, and `.170` passkey/voice-command coverage
  passed.
  A later 2026-07-17 Browser empty/gate copy pass replaced the no-page body,
  AccessKit empty summary, no-seat gate, missing-engine gate, spawn-failure gate,
  and incomplete Chromium gate with Browser-facing language instead of
  sandbox/helper/Servo/runtime/path wording; farm `.50` fmt, BigBoy `.130`
  focused empty-body paint coverage, `.90` live-helper empty-state coverage, and
  `.170` live-helper spawn/engine-gate coverage passed.
  A later 2026-07-17 Browser spelling copy pass kept raw Hunspell failure details
  in the spellcheck result state for diagnostics but moved the visible spelling
  status notice, drawer header summary, and warning row to Browser-facing
  dictionary/service language; farm `.50` fmt and BigBoy `.130` focused
  `spellcheck` coverage passed.
  A later 2026-07-17 Browser print copy pass moved print drawer labels,
  unavailable-printer notices, queued/failure print notices, and print-job
  completion text off CUPS/lp/spool-path wording while keeping the raw CUPS/lp
  helpers tested internally; farm `.50` fmt and BigBoy `.130` focused `print`
  coverage passed.
  A later 2026-07-17 Browser media-export copy pass renamed the visible Power
  menu media export row and status notices from media-manifest/spool wording to
  media-list/export language while preserving the internal JSON manifest format;
  farm `.50` fmt, BigBoy `.130` focused `media_export`, and `.90` focused Power
  menu coverage passed.
  A later 2026-07-17 Browser web-archive copy pass moved capture/offline-copy
  archive labels and status notices off MHTML wording while preserving the
  internal `.mhtml` archive format and save path; farm `.50` fmt, BigBoy `.130`
  focused offline-copy drawer coverage, `.90` focused menubar coverage, and
  `.170` focused capture-notice coverage passed.
  A later 2026-07-17 Browser offline-copy metadata pass moved the offline drawer
  and cached-body fallback off raw cache ids, tab indexes, engine labels, PNG
  format names, and timestamp-like counters while preserving the underlying
  cached snapshot state; farm `.50` fmt, BigBoy `.130` focused offline-copy
  drawer coverage, and `.90` focused cached-body coverage passed.
  A later 2026-07-17 Browser share/translation drawer metadata pass moved QR
  share and Translation drawers off raw request ids, peer hosts, matrix module
  terminology, tab indexes, and engine labels while preserving the underlying
  share route and translation result state; farm `.50` fmt, BigBoy `.130`
  focused QR drawer coverage, and `.90` focused Translation drawer coverage
  passed.
  A later 2026-07-17 Browser Site Styles copy pass moved the menu and drawer off
  injected/host/Userscripts implementation wording while preserving the CSS
  editor and site-style state; farm `.50` fmt, BigBoy `.130` focused Site
  Styles drawer coverage, and `.90` focused menubar coverage passed.
  A later 2026-07-17 Browser security/privacy copy pass moved safe-browsing and
  site-info surfaces off visible host/mesh-hosted wording while preserving host
  matching and policy-source state; farm `.170` fmt, BigBoy `.130` focused
  security panel coverage, `.90` focused Privacy menu coverage, and `.50`
  focused safe-browsing source coverage passed.
  A later 2026-07-17 Browser custom-filter policy pass made
  `browser/custom-filter-rules.txt` a real operator policy source instead of a
  test-only seam, publishes source-read status, keeps last-good rules active on
  missing/error, and shows the live custom-filter status in the Privacy menu;
  farm `.170` fmt, BigBoy `.130` focused custom-filter source coverage, `.90`
  focused Privacy menu coverage, and `.50` policy-source audit coverage passed.
  A later 2026-07-17 Browser synced-filter pass made
  `adfilter/compiled/engine.json` the real mesh-synced filter-list source for
  Browser blocking, preserves local operator custom rules while importing the
  compiled worker store, publishes source-read status, and replaces the static
  Privacy-menu filter-list promise with the live source count; farm `.170` fmt,
  BigBoy `.130` focused synced-filter source coverage, `.90` Privacy menu
  coverage, and `.50` policy-source audit coverage passed.
  A later 2026-07-17 Browser permission copy pass kept permission prompt
  decisions, Privacy menu captions, site-info panel text, and capture notices on
  user-facing blocked-by-default language while preserving the machine-readable
  permission enforcement events.
  A later 2026-07-17 Browser scrape-export engine-label pass kept JSON/CSV
  engine wire values stable while moving operator-facing Markdown exports off
  raw Servo wording and onto the same Lightweight engine label used in chrome.
  A later 2026-07-17 Browser offline-archive notice pass kept the saved archive
  format unchanged while moving the visible missing-archive notice off raw MHTML
  terminology.
  A later 2026-07-17 Browser download failure notice pass kept the transfer
  request staging paths unchanged while moving intercepted-download and
  observed-media/image download failures off raw spool/request-path terminology.
- Acceptance criteria: Command rows dispatch to real behavior; disabled items
  explain the gate; no text-only stub menu remains.
- Verification method: Focused command dispatch tests, print/capture tests, and
  live smoke for at least one download and one PDF/print path.
- Origin or merged source IDs: BROWSER-DD-8, BROWSER-DD-10, BROWSER-DD-12, old
  worklist lines 4161, 4207, 4232.

### WL-FUNC-005 - Unified search and omnibox indexing

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: Front-door and Browser omnibox search have app/mesh pieces, but the
  full unified search model needs file indexing, richer peer/service data, and
  AI-ranked candidates.
- Required outcome: Apps, files, mesh nodes/services, browser history/bookmarks,
  and assistant candidates share a local-first search index with clear ranking.
- Scope: File indexer, peer/service index, Browser omnibox integration, main
  input architecture, and privacy boundaries.
- Relevant files/components: Browser omnibox, Console/front-door search,
  Explorer/file services, assistant/AI surfaces.
- Dependencies: Search privacy policy and file indexer storage decision.
- Acceptance criteria: File results appear locally, mesh results rank by health
  and relevance, Browser omnibox can query the index, and private data stays local
  unless explicitly shared.
- Current evidence: A 2026-07-17 Start-menu console-search slice added static
  Console command candidates to the existing type-to-launch search, rendering
  them beside app results and dispatching them through `ConsoleState` so Goto,
  Plane, spawn, and gate behavior stay owned by the Console path; farm `.50`
  fmt, BigBoy `.130` focused Enter-launch coverage, and `.90` focused ranking
  coverage passed. A 2026-07-17 home-file search slice added a bounded local
  home snapshot to the Files model through the existing backend `list()` seam,
  merged it into the shell front door with active-folder de-duplication, and made
  path-backed results activate through Files even when the row was not already
  visible. Farm evidence: `.50` slot `home-search-fmt2`
  `cargo fmt -p mde-files-egui -p mde-shell-egui -- --check` passed after a
  formatter-only wrap; BigBoy `.130` slot `home-search-files`
  `cargo test -p mde-files-egui home_search -- --nocapture` passed 2/2; `.170`
  slot `home-search-files-reg`
  `cargo test -p mde-files-egui files_search_omnibox -- --nocapture` passed 1/1;
  and `.90` slot `home-search-shell-frontdoor`
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 5/5.
  A later 2026-07-17 Browser omnibox file-suggestion slice reused the Files
  model search rows as in-memory Browser suggestions, filtered them to path-backed
  local files, rendered them between bookmark and history rows, and committed
  them through the normal omnibox load path as `file://` destinations. Farm
  evidence: `.50` slot `browser-file-omnibox-fmt`
  `cargo fmt -p mde-shell-egui -- --check` passed; BigBoy `.130` slot
  `browser-file-omnibox-model2`
  `cargo test -p mde-shell-egui suggestion -- --nocapture` passed 9/9; and `.90`
  slot `browser-file-omnibox-chrome`
  `cargo test -p mde-shell-egui bookmark_suggestions_use_browser_painted_icons -- --nocapture`
  passed 1/1.
- Verification method: Index fixture tests and UI search regression.
- Origin or merged source IDs: SEARCH-omnibox, shell front-door search residual,
  old worklist line 6246.

### WL-FUNC-006 - Bottom navigation session entries and file-operation progress

- Status: Remaining
- Priority: P1
- Complexity: Medium
- Problem: User design direction requires active sessions as bottom-navigation
  entries and a reusable status area with progress bars inside the bottom
  navigation bar for all file operations.
- Required outcome: Remote desktop sessions appear as temporary bottom-bar
  entries, and every platform file operation can report progress through a shared
  bottom-nav status component.
- Scope: Bottom nav/taskbar model, VDI session entries, transfer/file operation
  progress API, progress rendering, and accessibility labels.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/dock.rs`,
  file/transfer services, VDI session state, shared `mde-egui` progress widgets.
- Dependencies: Current Win10 hybrid taskbar model.
- Current evidence: The 2026-07-17 progress pass verified that Files local
  operations, Browser downloads, Transfers jobs, and archive queue operations
  all fold into one bottom-rail `FileOperations` status projection, clicking it
  routes to Files → Transfers, and named Desktop sessions render as switchable
  bottom-rail entries. A later 2026-07-17 bottom-rail geometry pass strengthened
  the FileOperations proof so the progress pip and AccessKit node must remain
  inside the taskbar landmark and viewport; farm `.50` fmt and BigBoy `.130`
  focused bottom-rail progress coverage passed. A later 2026-07-17 shared
  progress-badge polish pass kept the operation status as a separate right-aligned
  text chip so percent/starting state remains visible with long labels, and
  replaced `operation(s)` AccessKit wording with normal singular/plural copy;
  farm `.50` fmt, BigBoy `.130` `mde-egui operation_progress_badge`, and `.90`
  `mde-shell-egui file_operation_progress` coverage passed. A live visual smoke
  is still needed before closing the item.
- Acceptance criteria: Opening a desktop creates a switchable bar entry; file
  copy/upload/download/compress/extract operations share the same progress UI;
  progress survives surface switches.
- Verification method: Shell unit tests for session entries and file operation
  progress fixtures, plus visual smoke.
- Origin or merged source IDs: NAVBAR-U3, TRANSFERS, user design answer
  2026-07-16, old worklist line 3302.

### WL-FUNC-007 - Media local video and library/art browse proof

- Status: Blocked
- Priority: P1
- Complexity: Medium
- Problem: Media/video has engine work, but live acceptance still needs proof
  that real mpv frames render to the Media stage on a seat and that library/art
  browsing works against a live source.
- Required outcome: Local video plays with visible frames, audio and controls on
  Eagle/test bed, and library browse/artwork paths work against the configured
  media service.
- Scope: mpv feature path, player stage, media library browser, artwork cache,
  live seat verification.
- Relevant files/components: `crates/desktop/mde-media-egui/`,
  `crates/desktop/mde-media-core/`, `crates/desktop/mde-shell-egui/`.
- Dependencies: Seat with libmpv and media library source.
- Acceptance criteria: Video frames advance, controls work, browse/artwork show
  real data, and missing engine paths show honest gated states.
- Verification method: Live seat smoke plus focused media tests.
- Origin or merged source IDs: BUG-VIDEO-1, MEDIA-VIDEO, MUSIC-BROWSE/ART,
  old worklist lines 3254, 6198, 1449.

### WL-FUNC-008 - Unified services view

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: Operators still need one truthful place to see canonical published
  services, probe-discovered services, and VM-internal service state.
- Required outcome: A unified Services view lists service source, endpoint,
  health, role, and action ownership without conflating discovery mechanisms.
- Scope: Services registry, probe discovery, VM-internal service view, UI, and
  service health.
- Relevant files/components: services flow, Explorer/Mesh views, media registry,
  OpenStack service catalog.
- Dependencies: Agreement on service record shape.
- Acceptance criteria: Published, discovered, and VM-internal services appear in
  one view with provenance and health; stale entries age out.
- Verification method: Fixture tests with mixed service sources and live registry
  smoke.
- Origin or merged source IDs: COMPUTE-DISCOVERY, old worklist line 1736.

## User Interface And Experience

### WL-UX-001 - Win10 hybrid bottom taskbar and tray live proof

- Status: Blocked
- Priority: P2
- Complexity: Medium
- Problem: The Win10 hybrid taskbar/start/tray work has many completed slices,
  but the remaining tray composition and live visual proof are still gated.
- Required outcome: The bottom taskbar, start grid, tray, show-desktop nub, and
  action center render without overlap on a live seat and match the canonical
  Quazar identity.
- Scope: Bottom bar geometry, tray/status area, action center, start grid,
  live-eye pass, and screenshots.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/dock/mod.rs`,
  `crates/desktop/mde-shell-egui/src/start_menu.rs`, status/system modules.
- Dependencies: Live DRM seat for final visual proof.
- Acceptance criteria: No overlaps at supported resolutions; tray controls are
  reachable; live screenshots confirm layout.
- Verification method: Focused geometry tests and live seat screenshot/pixel
  inspection.
- Origin or merged source IDs: B5-rest, WIN10-HYBRID, old worklist line 4630.
- Evidence 2026-07-17: Start menu pinned/favorite tiles now persist through
  `start-menu.json` in the shell client-data directory; malformed, duplicate,
  unknown, and non-grid pins normalize on load. Live tray/screenshot proof remains
  the blocking tail for this item. A later 2026-07-17 Start-menu geometry pass
  moved the panel off the retired left-dock `DOCK_W` inset and back to the true
  screen-left edge, matching the bottom-taskbar-only architecture. A later
  2026-07-17 taskbar hover-preview pass added the static running-session preview
  with a real protocol badge above the taskbar; farm `.50` fmt and BigBoy `.130`
  focused `win10_hybrid_31_session_hover_preview_shows_protocol_badge` passed.
  A later 2026-07-17 taskbar live-thumbnail pass wired the current VDI desktop
  texture into that hover preview, preserving aspect ratio and matching only the
  intended broker/fallback rail entry; farm `.50` fmt, BigBoy `.130` focused
  `session_preview`, and the exact hover-card regression passed. A later
  2026-07-17 taskbar auto-hide settings pass made the already-tested dock
  auto-hide behavior reachable from the persisted Personalization appearance
  config and mirrored it into `DockState`; farm `.50` fmt, BigBoy `.130`
  focused `appearance`, and the edited legacy migration test passed.
  A later 2026-07-17 Start-menu pinned-layout pass bounded the pinned/grouped
  tile grid to the viewport above the fixed search field with a vertical scroll
  region, preventing pinned sections from painting into search; farm `.50` fmt
  and BigBoy `.130` focused pinned-layout coverage passed.
  A later 2026-07-17 source-comment hygiene pass aligned `main.rs` and
  `dock/mod.rs` with the bottom-taskbar-only architecture, removing stale live
  source prose that still described a mounted left vertical dock; farm `.50`
  fmt and BigBoy `.130` focused retired-gutter coverage passed.
  A later 2026-07-17 Start-menu source-doc pass removed stale placeholder and
  vertical-dock-launcher prose from `start_menu.rs` and the shell `Nav` comment,
  aligning the code comments with the shipped tile/search/pin Start Menu; farm
  `.50` fmt and BigBoy `.130` focused Start-menu grid coverage passed.
  A later 2026-07-17 Start-menu search-icon pass added reusable `ui-search` and
  `ui-close` line glyphs, rendered a leading search icon plus live query-clear
  icon button in the Start search field, and exposed the clear button to
  AccessKit; farm `.50` fmt, BigBoy `.130` focused clear-button coverage, and
  `.90` `mde-theme` icon rasterization coverage passed.

### WL-UX-003 - Accessibility consumer and application sweep

- Status: Remaining
- Priority: P2
- Complexity: Epic
- Problem: The DRM AccessKit bridge and reduce-motion plumbing now exist, but a
  complete accessibility posture still needs a real consumer/screen-reader path,
  app-level annotations, toast live regions, and companion app coverage.
- Required outcome: The shipped DRM seat can expose a usable accessibility tree
  to an assistive consumer, and major shell/app surfaces have labels, roles,
  focus, live regions, and reduce-motion behavior.
- Scope: AccessKit consumer/TTS decision, app-picker/system quad, toasts,
  Explorer, curtain, VDI, Device Manager, Chooser, companion apps, and tests.
- Relevant files/components: `crates/shared/mde-egui/src/a11y.rs`,
  `crates/shared/mde-egui/src/drm.rs`, `crates/desktop/mde-shell-egui/src/`,
  companion egui crates.
- Dependencies: Accessibility output strategy; governance currently marks broad
  accessibility as deferred for the cutover.
- Acceptance criteria: `MDE_A11Y=1` or a persisted setting produces a consumable
  tree, critical toasts use live regions, raw-painted cells have names/roles, and
  reduce-motion reaches auto-rotating surfaces.
- Current evidence: A 2026-07-17 Start-menu reduced-motion pass made live-tile
  rotation read the system Appearance motion signal, freezes multi-fact tiles on
  their primary fact when motion is reduced/disabled, suppresses the rotating
  tile live region while frozen, and stops the settled-open rotation heartbeat;
  farm `.170` fmt, BigBoy `.130` focused Start-menu coverage, `.90` system
  motion-setting coverage, and `.50` shared motion coverage passed.
  A later 2026-07-17 Start-menu context-row AccessKit pass added named button
  nodes for the tile context menu's Open/Pin rows; farm `.50` fmt and BigBoy
  `.130` focused context-row coverage passed.
- Verification method: AccessKit tree tests, live consumer smoke, and UI tests for
  named controls.
- Origin or merged source IDs: a11y-02/04/05/06/07/08, shell-ux-6, platform
  review accessibility cluster.

## Performance

### WL-PERF-001 - VDI dirty-rectangle display uploads

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: VDI display paths avoid some idle work, but changed frames can still
  upload full framebuffers instead of dirty sub-rectangles.
- Required outcome: VDI transports carry damage information to the shell and
  upload only changed regions where supported, with honest fallback to full-frame.
- Scope: SPICE/VNC/RDP frame metadata, `mde-vdi-core` image deltas, shell texture
  updates, and live visual validation.
- Relevant files/components: `crates/desktop/mde-vdi-core/`,
  `crates/desktop/mde-vdi-spice/`, `crates/desktop/mde-shell-egui/src/vdi.rs`.
- Dependencies: Stable delta API and transport support.
- Acceptance criteria: Dirty-rect transports update subregions, full-frame
  fallback remains correct, and visual output is unchanged.
- Verification method: Unit tests for ImageDelta plus live performance/visual
  smoke.
- Origin or merged source IDs: platform review `perf-7`, open ledger partial.

### WL-PERF-002 - Seat responsiveness residuals

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: The DRM present loop is now event-driven, but media/browser/VDI frame
  producers must reliably wake the seat without pointer movement, and periodic
  probes must not reintroduce render-thread stalls.
- Required outcome: All live frame producers request repaints correctly, and slow
  hardware probes stay off the render thread.
- Scope: Browser frame pump, media stage, VDI frames, seat snapshot pump, DDC/PipeWire
  probes, and repaint scheduling.
- Relevant files/components: `crates/shared/mde-egui/src/drm.rs`,
  `crates/desktop/mde-shell-egui/src/seat_pump.rs`,
  Browser/media/VDI frame paths.
- Dependencies: Browser-specific idle playback was proven by archived
  WL-CRIT-003; remaining proof should cover non-Browser media/VDI frame sources
  and slow probe isolation.
- Current evidence: A 2026-07-17 Browser PiP repaint pass added a background
  Browser media heartbeat for playing PiP tabs, including the active-internal-page
  regression where the previous active-page-only heartbeat would not keep polling
  frames; farm `.50` fmt, BigBoy `.130` focused `browser_media_pip`, and `.90`
  active-page heartbeat tests passed.
- Acceptance criteria: No frame source requires pointer movement to advance; slow
  probes cannot freeze UI; regression tests cover wake scheduling.
- Verification method: Headless wake tests plus live seat smoke.
- Origin or merged source IDs: platform review `perf-1`, `perf-2`, user video
  freeze report.

## Testing And Quality

### WL-TEST-001 - OpenStack live and contract tests

- Status: Remaining
- Priority: P1
- Complexity: Medium
- Problem: The OpenStack worker now has contract fixtures, but live-gated smoke
  against a real farm/dev cloud remains necessary for the resource-creating path.
- Required outcome: A gated live suite authenticates, lists resources, creates a
  tiny throwaway server or equivalent harmless resource, and deletes it.
- Scope: Env-gated tests, captured real JSON fixtures, farm lane, cleanup safety,
  and docs.
- Relevant files/components: `crates/mesh/mackesd/src/workers/openstack/`,
  contract fixtures, farm scripts.
- Dependencies: Farm OpenStack endpoint and throwaway project quota.
- Acceptance criteria: Contract tests replay real fixtures; live ignored test
  passes when `MDE_OPENSTACK_LIVE` is set; cleanup runs on failure.
- Verification method: `cargo test` contract suite and operator/farm live smoke.
- Origin or merged source IDs: platform review `test-obs-5`, QC-16.

### WL-TEST-002 - Crown-jewel integration harness for real etcd/Nebula

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Real-etcd election and real-Nebula overlay integration suites exist as
  concepts but need a runnable harness on the airgapped farm.
- Required outcome: The farm can spin disposable mesh nodes or equivalent
  fixtures to run election, overlay, enroll, and recovery tests without manual
  setup.
- Scope: Harness, fixtures, teardown, farm scheduling, logs/artifacts, and docs.
- Relevant files/components: `install-helpers/xcp-build.sh`, farm scripts,
  substrate/election tests, Nebula test helpers.
- Dependencies: Farm VM capacity and approved destructive test boundaries.
- Acceptance criteria: Harness creates isolated nodes, runs tests, tears down or
  reverts snapshots, and produces artifacts for failures.
- Verification method: One full harness run on farm, not repeated as filler.
- Origin or merged source IDs: platform review `test-obs-6`,
  `docs/BUILD-ENVIRONMENT.md`.

## Documentation And Maintenance

### WL-DOC-001 - Stale architecture/design docs need supersession banners

- Status: Remaining
- Priority: P2
- Complexity: Medium
- Problem: Historical design docs still mention retired COSMIC/iced/Carbon,
  mde-workbench, or cloud-hypervisor paths without consistently stating whether
  they are historical or current.
- Required outcome: Current docs point to egui-native, Nova/libvirt/QEMU-KVM, and
  the active worklist; historical docs either move to archive or carry a
  supersession banner.
- Scope: README, architecture/design docs, diagrams, router-control docs, old
  survey docs, and cross-links.
- Relevant files/components: `README.md`, `docs/architecture.md`,
  `docs/design/`, `docs/diagrams/`, `AI_GOVERNANCE.md`.
- Dependencies: Brand decision for final spelling-sensitive docs.
- Acceptance criteria: Current operator docs do not instruct against retired
  architecture; historical docs are clearly labeled; no stale worklist pointer
  forks tracking.
- Verification method: Grep for retired terms with allowlisted historical files.
- Origin or merged source IDs: docs review `docs-consistency-1`,
  `docs-consistency-3`, `docs-consistency-6`, `docs-consistency-9`, repo scan.

### WL-DOC-002 - Re-key operator queue to reconciled IDs

- Status: Remaining
- Priority: P2
- Complexity: Small
- Problem: `docs/NEEDS-OPERATOR.md` is a useful blocked-work queue, but it still
  names many old IDs and should point to the new authoritative WL IDs.
- Required outcome: Operator-facing blocked items reference this worklist's IDs,
  with stale or already-closed entries archived.
- Scope: `docs/NEEDS-OPERATOR.md`, archive links, and blocked status mapping.
- Relevant files/components: `docs/NEEDS-OPERATOR.md`,
  `docs/worklist-archive/2026-07-16-reconciliation-archive.md`.
- Dependencies: This reconciliation landing.
- Acceptance criteria: Every operator-blocked item maps to a WL ID or is archived
  with a reason; no old tracker is presented as active.
- Verification method: Manual review plus grep for old-only open IDs.
- Origin or merged source IDs: `docs/NEEDS-OPERATOR.md`, user request to move old
  `docs/WORKLIST.md` contents into the correct platform worklist.

### WL-DOC-003 - Active worklist stewardship

- Status: Remaining
- Priority: P3
- Complexity: Small
- Problem: Agents need a stable process for adding, completing, merging, and
  archiving worklist items without repeating the giant-file failure.
- Required outcome: Document the lifecycle: new IDs, required fields, when to
  archive, how to cite evidence, and how to avoid duplicate workstreams.
- Scope: Worklist header, agent docs, archive README, and lint instructions.
- Relevant files/components: `docs/platform/WORKLIST.md`, `docs/worklist-archive/`,
  `AGENTS.md`, `AI_GOVERNANCE.md`.
- Dependencies: `install-helpers/lint-worklist.sh` for enforceable checks.
- Acceptance criteria: A future agent can close or add an item without inventing
  a parallel tracker or leaving closed work in the active file.
- Verification method: Documentation review and worklist lint fixture.
- Origin or merged source IDs: docs review `docs-consistency-10`, line-divergence
  postmortem.
