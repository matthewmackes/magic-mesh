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

### WL-ARCH-001 - Construct Cloud provider-neutral runway and OpenStack exit

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Construct Cloud is still coupled to OpenStack service names, Kolla
  topology, Nova/Heat resource verbs, and `state/openstack/*` mirrors while the
  user has directed a new track to move away from OpenStack. The existing
  backend must remain honest and usable until a replacement provider path carries
  the same typed behavior.
- Required outcome: Cloud UI, Bus verbs, persisted mirrors, orchestration forms,
  image/network lifecycle, and docs use provider-neutral Construct Cloud
  contracts; OpenStack becomes a replaceable backend adapter instead of the
  product architecture, and replacement provider work can be introduced without
  rewiring shell surfaces.
- Scope: Cloud provider contracts, OpenStack adapter boundaries, provider
  registry/configuration, IaC UI labels and verbs, image pipeline, networking,
  old-stack deletion after replacement proof, docs cleanup, and live cloud
  status.
- Relevant files/components: `crates/mesh/mackesd/src/workers/openstack/`,
  `docs/design/quasar-cloud.md`, `docs/ops/quasar-cloud-runbook.md`,
  `packaging/bootc/`, cloud UI, `crates/desktop/mde-shell-egui/src/iac/`, and
  unit-aggregator cloud mirror consumers.
- Dependencies: Replacement provider decision/prototype, farm dev cloud/test bed,
  and live cloud credentials.
- Current evidence: A 2026-07-18 provider-neutral runway pass updated
  `AI_GOVERNANCE.md` with the newer Construct Cloud provider-neutral lock, then
  moved the native IaC surface's user-facing copy from OpenStack/Keystone/Heat/
  HOT wording to Construct Cloud, Cloud provider, Cloud API status,
  Orchestration, and Template language while preserving backend diagnostics and
  existing wire contracts. Farm evidence: BigBoy `.130` slot
  `openstack-exit-iac`
  `cargo test -p mde-shell-egui iac -- --nocapture` passed 31 tests; focused
  rustfmt over the three edited IaC files passed. Whole-crate `cargo fmt
  --package mde-shell-egui -- --check` is still blocked by unrelated existing
  formatting drift in other dirty shell files. A follow-up 2026-07-18
  unit-aggregator slice made the Bus cloud mirror reader prefer provider-neutral
  `state/cloud/<node>` mirrors while accepting legacy `state/openstack/<node>`
  adapter mirrors for backward-compatible reads and diagnostics. Farm evidence:
  the post-cleanup `.50` slot `openstack-exit-units3`
  `cargo test -p mackesd unit_aggregator::sources -- --nocapture` passed 8
  focused source tests, including persisted cloud+legacy topic folding into
  units and invalid/empty topic rejection after disambiguating the provider-
  neutral `CloudMirrorSource::read` call from the legacy compatibility trait;
  `.90` slot `openstack-exit-fmt3` `cargo fmt -p mackesd -- --check` passed.
  A third 2026-07-18 OpenStack-exit track added the provider-neutral
  `mackes_mesh_types::cloud` facade so new consumers can import Construct Cloud
  catalog, health, resource table, and orchestration aliases without binding to
  the legacy `openstack` module path. The facade accepts direct provider-neutral
  catalog/resource JSON from a non-OpenStack fake while preserving Keystone and
  OpenStack collection fallback parsing for the installed adapter. Farm
  evidence: `.90` slot `cloud-facade-test`
  `cargo test -p mackes-mesh-types cloud -- --nocapture` passed 6 focused tests;
  `.50` slot `cloud-facade-fmt`
  `cargo fmt -p mackes-mesh-types -- --check` passed.
- Acceptance criteria: User-facing shell surfaces no longer require OpenStack,
  Keystone, Nova, Heat, or Horizon terminology; typed Bus and persisted cloud
  contracts can be satisfied by at least one non-OpenStack fake/provider in
  tests; the existing OpenStack backend can be disabled without breaking the UI;
  a replacement backend can list and launch a test workload over mesh networking;
  stale OpenStack-only docs are archived or bannered once the replacement path is
  live.
- Verification method: Provider-neutral UI and contract fixture tests, OpenStack
  adapter compatibility tests while it remains installed, provider-disabled UI
  smoke, replacement-provider smoke, and `/audit` grep for product-facing
  OpenStack terminology.
- Origin or merged source IDs: QC-1 through QC-15, OW-8, E12 supersession notes,
  old worklist lines 3457-3567, user directive 2026-07-18 to start moving away
  from OpenStack.

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

## Runtime Reliability

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
  A later 2026-07-18 Browser page-actions popup proof pass tightened the
  rendered regression for the toolbar bookmark/page-actions menu: collapsed
  context entry points and open toolbar popups must expose bounded AccessKit
  button rows, paint Browser Chrome text, and settle to the Browser popup
  surface instead of a thin wedge or inherited shell-dark surface. Farm
  evidence: `.50` isolated and combined
  `cargo test -p mde-shell-egui page_actions -- --nocapture` passed 7 tests;
  `.90` combined `cargo fmt -p mde-shell-egui -- --check` passed.
  A follow-up 2026-07-18 Browser bookmark-overflow popup pass reserved the
  toolbar popup width before rendering the Browser chrome frame, preventing
  right-aligned toolbar layout from squeezing overflow bookmark rows into a
  thin wedge. Farm evidence: `.90` focused
  `cargo test -p mde-shell-egui bookmark_overflow -- --nocapture` passed, and
  `.50` `cargo fmt -p mde-shell-egui -p mde-files-egui -- --check` passed.
- Acceptance criteria: A tab/bookmark/settings change on node A appears on node B,
  conflicts converge, and Browser does not maintain a competing bookmark store.
- Verification method: Multi-node sync test or deterministic two-store fixture,
  plus Browser UI regression.
- Origin or merged source IDs: BROWSER-DD-7, user decision "Use system bookmark
  manager", old worklist line 4139.

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
  A later 2026-07-18 Browser location-bar usability slice raised the Location
  edit field height/text metrics and clears only the committed page URL when a
  user begins a fresh omnibox edit, while preserving partially typed drafts.
  Farm evidence: BigBoy `.130` slot `browser-omnibox-clear`
  `cargo test -p mde-shell-egui omnibox -- --nocapture` passed 25 tests.
  A later 2026-07-18 Browser omnibox polish slice raised the active Location
  text to a larger Browser-local scale, gave the field a taller row/inner text
  budget, and tightened the readability guard so it cannot regress to dense
  toolbar typography. Farm evidence: BigBoy `.130` slot
  `browser-omnibox-polish` focused
  `cargo test -p mde-shell-egui browser_omnibox -- --nocapture` passed 3 tests;
  `.50` slot `browser-omnibox-polish-fmt` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` for
  `web/mod.rs` and `web/chrome_ui/mod.rs` passed. Package-level fmt is still
  blocked by unrelated dirty formatting drift in other shell files, so it was
  not used as a claim for this slice.
  A later 2026-07-19 Browser omnibox clipping slice made the unfocused pretty
  URL overlay and focused inline-completion tail paint through the Location
  field clip rect, so long URLs cannot overpaint right-side Browser controls.
  Farm evidence: `.90` slot `start-menu-light-style-test`
  `cargo test -p mde-shell-egui omnibox -- --nocapture` passed 27 tests; `.50`
  file-scoped `rustfmt --edition 2024 --config skip_children=true --check` for
  `web/chrome_ui/mod.rs` passed.
  A later 2026-07-19 Browser suggestion-hover polish slice routed bookmark,
  file, history, and web-search suggestion chip hovers through the shared Browser
  `chrome_hover_text` primitive and made the rendered hover tooltip prove Browser
  Chrome text/surface colors. The bookmark-suggestion icon regression now accepts
  both YAMIS image meshes and vector fallback strokes. Farm evidence: `.90` slot
  `start-menu-light-style-test`
  `cargo test -p mde-shell-egui suggestion -- --nocapture` passed 11 tests; `.50`
  file-scoped `rustfmt --edition 2024 --config skip_children=true --check` for
  `web/chrome_ui/mod.rs` passed.
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
  `mde-shell-egui file_operation_progress` coverage passed. A later 2026-07-17
  compact-rail visual pass made active file operations reserve a mini progress
  badge directly inside the bottom navigation status cluster instead of only a
  Files pip, kept the expanded status-panel progress row, and added screenshot
  proof for both states; BigBoy `.130` focused `file_operation_progress`
  coverage passed, farm `.50` fmt passed, and the generated
  `taskbar-file-progress-rail.png` / `taskbar-file-progress-panel.png` artifacts
  were pulled and visually inspected. A follow-up 2026-07-18 progress-pump slice
  made the shell pump Files transfers and Browser downloads before rendering the
  shared bottom-rail status segment, so progress stays current while other
  workspaces are active; BigBoy `.130` focused `shell_taskbar_pumps_` coverage
  passed. A follow-up 2026-07-18 Fedora 44 split-RPM proof from commit
  `13844e25` installed the bounded taskbar-progress/browser-download pump slice
  on `.15` with clean `rpm -V magic-mesh magic-mesh-browser`, matched running
  shell hash, and passed installed Browser all-engine, link-navigation,
  idle-media, Google, and Google News smokes. This is package/runtime proof; a
  live `.15` screenshot-level visual smoke of the bottom rail remains needed
  before closing the item.
- Acceptance criteria: Opening a desktop creates a switchable bar entry; file
  copy/upload/download/compress/extract operations share the same progress UI;
  progress survives surface switches.
- Verification method: Shell unit tests for session entries and file operation
  progress fixtures, plus visual smoke.
- Origin or merged source IDs: NAVBAR-U3, TRANSFERS, user design answer
  2026-07-16, old worklist line 3302.

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

### WL-UX-005 - Start Menu and Front Door launcher overhaul epic

- Status: Remaining
- Priority: P2
- Complexity: Epic
- Problem: Start Menu, Front Door, dock launcher taxonomy, and shared search
  work are documented across multiple evidence sources. The current code already
  has a Start Menu, shell Front Door panel, and shared search ranker slices, but
  the full launcher overhaul is not tracked as one active source of truth. This
  leaves major requirements scattered: one coherent launcher, consistent grouping,
  fuzzy type-to-launch, real app icons, favorites, mesh/workload/service filters,
  peer app discovery, Front Door panel mode, and retirement of duplicate launcher
  surfaces.
- Required outcome: The Start button and Super key open one polished Front Door
  launcher experience that serves as the local Start menu and mesh-wide launcher,
  with compact panel mode, optional full-screen expansion, favorites, unified
  search, consistent grouping, real icons, keyboard-first operation, and no
  duplicate application-launcher surface left at parity.
- Scope: Start Menu to Front Door consolidation; panel/full-screen layout;
  shared launcher taxonomy; app, Console, file, Browser, mesh, peer-app,
  workload, and service result rows; favorites/pins; search filters/chips;
  keyboard navigation; focus-or-launch behavior; peer app on-demand query and
  remote-desktop launch; real icon-theme/YAMIS integration; accessibility;
  reduce-motion; live visual proof; migration/removal of retired launcher entry
  points only after parity.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/start_menu.rs`,
  `crates/desktop/mde-shell-egui/src/front_door.rs`,
  `crates/desktop/mde-shell-egui/src/dock/mod.rs`,
  `crates/desktop/mde-shell-egui/src/main.rs`,
  `crates/shared/mde-egui/src/search_omnibox.rs`,
  Console, Chooser, Browser suggestions, Files search rows, Explorer search,
  `mackesd` app/workload/service inventory paths, and icon-theme/YAMIS assets.
- Dependencies: WL-FUNC-005 for shared search/indexing; WL-UX-001 for final live
  screenshot proof of the surrounding bottom bar/tray geometry; WL-UX-003 for
  broad accessibility consumer proof; peer-app discovery requires a local app
  inventory/RPC seam; launcher retirement waits for parity.
- UX workflow:
  1. **Summon and orient:** Start button or Super opens the Front Door panel on
     the primary monitor, focused in the search field but still showing the
     favorites grid, local status, and available filter chips without requiring
     the user to type first.
  2. **Local-first launch:** typing filters local apps and commands immediately;
     the top match is selected, Enter activates it, and app activation
     focus-or-launches through the existing shell surface route.
  3. **Browse without typing:** Apps, Mesh, Workloads, Services, Files, Browser,
     and Commands filters expose scannable rows/cards with real icons, short
     labels, one-line context, health/state pills where relevant, and no hidden
     web-wrapper behavior.
  4. **Act in place:** workload/service rows expose safe inline actions only
     where a real seam exists; destructive actions use the platform arming or
     typed-confirm pattern, and long operations report through the shared
     bottom-navigation progress/status area.
  5. **Peer app path:** selecting the Mesh/peer context lazy-loads that peer's
     app set, shows on-peer badges, and launches through the approved remote
     desktop path without blocking local launcher responsiveness.
  6. **Manage favorites:** pin/unpin and reorder are available from the same row
     context grammar as the Start pinned tiles; favorites remain local/offline
     usable and are mesh-sync ready once the storage seam exists.
  7. **Recover gracefully:** no local results, mesh-down, slow peer, missing tool,
     and disabled action states show honest empty/degraded rows with clear next
     actions instead of blank space or inert controls.
  8. **Proof the experience:** each implementation slice captures the affected
     workflow with focused tests plus rendered proof when layout changes; final
     acceptance uses `.15` and the Sunshine/Moonlight proof surface when
     available, checking panel mode, full-screen mode, keyboard-only operation,
     text/icon legibility, focus visibility, and absence of overlap.
- Acceptance criteria:
  1. Start button and Super open Front Door panel mode on the primary monitor in
     under 150 ms from cached local data.
  2. Panel mode presents favorites first, supports full-screen expansion, and
     remains usable at supported portrait/tablet and desktop sizes without
     overlap.
  3. Typing immediately fuzzy-matches local apps by name, keywords, and
     description; Console commands, files, Browser history/bookmarks, mesh units,
     workloads, and services appear through the shared result model.
  4. `>` command input routes through an explicit run-command path, not a fake
     search result.
  5. Result activation dispatches through each owner surface's existing action
     seam: app focus-or-launch, Console actions, Files open, Browser load,
     Explorer jump, workload/service action, or peer remote-desktop launch.
  6. Dock and launcher use one truthful grouping/taxonomy source; Browser and
     Bookmarks are not categorized as Terminals, Files is not categorized as
     System, and a unit test fails on divergence.
  7. Local entries show real app icons where available and YAMIS/Carbon-style
     platform glyphs for mesh, workload, service, and command rows.
  8. Keyboard operation covers open, type, filter-chip traversal, arrow
     navigation, Enter activation, Escape close, and selected-row visibility.
  9. Mesh-down state hides or gates mesh results without slowing local launcher
     open; slow peer app discovery never blocks the panel.
  10. Favorites/pins persist per user and are ready for mesh sync when that
      storage seam is available; no usage-history tracking is introduced.
  11. AccessKit labels/roles/values exist for the search field, filter chips,
      favorite tiles, result rows, and expansion controls; reduce-motion disables
      nonessential entrance/rotation motion.
  12. Any retired launcher/applet entry point is removed only after parity tests
      prove the Front Door path covers its launch behavior.
- Verification method: Focused unit tests for taxonomy agreement, ranking,
  dispatch seams, favorites persistence, keyboard navigation, AccessKit rows, and
  peer-discovery gating; farm `mde-shell-egui` targeted tests plus relevant
  `mde-egui`, Files, Browser, Explorer, and `mackesd` tests; live `.15` rendered
  proof with screenshot/pixel inspection for panel/full-screen modes and
  Start/Super open behavior.
- Current evidence: 2026-07-18 Start/Front Door slice moved the active launcher
  grouping into `dock::LAUNCHER_GROUPS`, made Start consume that shared taxonomy,
  and changed Front Door app rows to show the shared group label instead of
  `surface:*` debug targets. Front Door result rows now paint YAMIS-backed icons
  for app, file, mesh, bookmark, history, web, and assistant domains through the
  existing `IconId` loader. Farm verification used only targeted lanes:
  `.130` `cargo test -p mde-shell-egui front_door -- --nocapture` passed 11
  tests; `.170` passed
  `start_tiles_use_the_shared_launcher_taxonomy_source`; `.90` passed
  `the_19_surfaces_are_grouped_into_lock_8s_7_function_based_groups`; `.50`
  passed `cargo fmt -p mde-shell-egui --check`. A follow-up 2026-07-18 panel
  mode slice made blank Front Door show initial local shortcut rows instead of
  an empty `Type to search` body, reusing Start's persisted pin order as
  read-only display priority and leaving Start as the owner of pin mutation and
  persistence. Nonblank queries still route through the shared omnibox ranker.
  BigBoy `.130` passed `cargo test -p mde-shell-egui front_door -- --nocapture`
  with 15 focused tests; `.50` passed `cargo fmt -p mde-shell-egui --check`.
  A later 2026-07-18 Start/Super routing slice moved the primary Start button
  and clean Super tap launcher path onto the unified Front Door panel while the
  legacy Start Menu remains mounted only for compatibility drains until parity
  retirement. The taskbar Start cell now mirrors the active Front Door launcher
  state and exposes "Start launcher" accessibility state text. Farm evidence:
  BigBoy `.130` focused `front_door` suite passed with 16 tests, including
  `start_launcher_toggle_opens_front_door_not_legacy_start_menu`; `.170` current
  source `cargo fmt -p mde-shell-egui --check` passed; `.90` focused
  `start_launcher_toggle_opens_front_door_not_legacy_start_menu` passed.
  A later 2026-07-18 Front Door filter-chip slice added compact All, Apps,
  Files, Mesh, Browser, and Web chips under the launcher search box. The chips
  filter the existing shared `SearchDomain` buckets instead of adding a second
  taxonomy, keep blank and typed result paths on the existing owner dispatch
  seams, and export clickable AccessKit buttons with selected state. Farm
  evidence: BigBoy `.130` focused `front_door` suite passed with 18 tests; `.90`
  focused `front_door_filter_chips_keep_domains_separate` passed; `.170`
  focused `front_door_filter_chips_export_accesskit_buttons` passed; `.50`
  current-source `cargo fmt -p mde-shell-egui --check` passed. A later
  2026-07-18 full-screen expansion slice added a compact YAMIS icon layout
  toggle beside the Front Door search field, kept panel mode as the default
  Start/Super entry state, and added an expanded bounded launcher canvas for
  desktop and portrait/tablet sizes without changing the shared result or
  activation model. The layout toggle exports AccessKit button state with
  `Panel` / `Full-screen` values and selected state in expanded mode. Farm
  evidence: BigBoy `.130` focused `front_door` suite passed 20 tests; `.90`
  focused `front_door_expanded_layout_uses_bounded_screen_geometry` passed;
  `.170` focused `front_door_expansion_control_exports_accesskit_state` passed;
  `.50` `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18
  Front Door narrow-panel polish slice made the search row, filter chips, and
  result rows size against the actual panel clip width. Filter chips now scale
  and clip label paint inside their chip rects, cramped search rows stop forcing
  the expansion button past the row budget, result domain badges shrink within
  a bounded range, and title/target text is clipped to the remaining row text
  column. Farm evidence: BigBoy `.130` focused `front_door` suite passed 21
  tests; `.90` focused `front_door_narrow_panel_chips_and_rows_stay_bounded`
  passed; `.170` focused `front_door_filter_chips_export_accesskit_buttons`
  passed; `.50` `cargo fmt -p mde-shell-egui --check` passed. A later
  2026-07-18 Front Door selected-action slice moved mouse interaction toward
  the locked click-to-expand launcher design: row clicks now select/expand the
  result instead of immediately activating it, the selected row renders a
  compact primary action strip, and the primary button plus Enter continue to
  dispatch through the existing owner `FrontDoorTarget` seams. The selected
  action exports an AccessKit button with domain and target metadata, and the
  result-list height model reserves space for the strip so narrow/short panels
  remain bounded. Farm evidence: BigBoy `.130` focused `front_door` suite passed
  22 tests; `.90` focused
  `front_door_selected_result_exports_primary_action_button` passed; `.170`
  focused `front_door_narrow_panel_chips_and_rows_stay_bounded` passed; `.50`
  `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18 Front Door
  command-mode slice added explicit `>` run-command mode: `>` input bypasses the
  ranked app/file/browser result list, renders one bounded command row plus a
  primary Run action, exports command-row and Run-action AccessKit buttons, and
  returns a distinct `FrontDoorTarget::RunCommand`. Shell activation routes that
  target through Console's typed `SpawnTab` recipe and the embedded Terminal
  surface instead of fabricating a launcher result. Farm evidence: BigBoy `.130`
  focused `front_door` suite passed 26 tests, including the new command-mode,
  Console recipe, and shell terminal-route coverage; `.50` `cargo fmt --check`
  passed. A later 2026-07-18 Front Door command-candidate slice added static
  Console rows to the shared Front Door result model, added a compact Commands
  filter chip, renders those rows as `Command` domain results with Console-owned
  icons/AccessKit metadata, and activates them through
  `ConsoleState::activate_index` plus the shared shell Console request drain.
  Farm evidence: BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 29 tests,
  including command-row AccessKit and Console activation coverage; `.50`
  `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18 Front Door
  local Workloads/Services filter slice added Workloads and Services chips to
  the same filter row. These intentionally expose only the current local owner
  surfaces (`Desktop` / `Infra as Code` for workloads, `Workbench` /
  `Infra as Code` for service workflows) and add workload/service keywords to
  local app search; peer app discovery, service cards, and inline
  start/stop/restart actions remain open parity work. Farm evidence: BigBoy
  `.130` focused `cargo test -p mde-shell-egui front_door -- --nocapture`
  passed 29 tests, including the broadened filter scope/search coverage; `.50`
  `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18 Front Door
  workflow-card slice added distinct Workload and Service result cards for the
  current local owner surfaces instead of repurposing app rows: Cloud workloads
  and Desktop sessions route to Infra as Code/Desktop, while Mesh services and
  Cloud API services route to Workbench/Infra as Code. These cards use their own
  result domain labels, YAMIS-backed icons, search terms, AccessKit metadata, and
  primary Open action through the existing owner-surface switcher. Unit-specific
  start/stop/restart controls remain open until a safe lifecycle backend seam
  exists. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 31 tests; a
  duplicate `.90` workflow filter lane was stopped after BigBoy covered the same
  assertions, per the no-filler-retest rule. A later 2026-07-18 Front Door
  favorites-management slice added a typed Front Door request model and a
  selected-app Pin/Unpin action, plus matching row context menu entries, wired
  to the existing Start Menu persisted pin store instead of creating a second
  preference file. Pin/unpin requests keep the launcher open for continued
  favorites management, while Launch/Open/Run still close through the existing
  owner activation seams. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 33 tests,
  including AccessKit Pin/Unpin button metadata and Shell pin-store routing. A
  follow-up 2026-07-18 ordered-favorites slice added Move up/Move down
  Front Door requests for already-pinned app rows, renders bounded icon-first
  reorder controls in the selected action strip, mirrors the same operations in
  the row context menu, and routes them through the Start-owned persisted pin
  order. Farm evidence: BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 33 tests,
  including AccessKit reorder metadata and Shell order mutation; `.50`
  `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18
  Front Door mesh-source gating slice added an explicit
  `FrontDoorSourceStatus` model, makes the Shell pass Explorer's cached mesh
  source state into the launcher, and gates Mesh-domain rows before selection or
  Enter activation when peer data is unavailable or still warming. The launcher
  does not poll or block on Explorer/Bus discovery; it uses cached metadata only
  and renders an honest non-actionable degraded status row with AccessKit live
  status while local app, command, file, and Browser results remain available.
  Peer app lazy-load and remote-desktop launch remain open parity work. Farm
  evidence: BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 35 tests,
  including mesh-source gating and degraded-source AccessKit coverage; `.90`
  focused
  `cargo test -p mde-shell-egui explorer_search_items_feed_the_shared_ranker_for_unit_fields -- --nocapture`
  passed; `.170` `cargo fmt -p mde-shell-egui --check` passed. A later
  2026-07-18 Front Door peer Desktop-connect slice added a selected-row
  `Connect` action and context-menu row for mesh peer desktop source ids
  (`peer:` / `peer-vm:`), while keeping the normal primary `Open` action routed
  to Explorer focus. The new request routes through
  `ChooserState::connect_source_id` and hands any returned request to the VDI
  surface with the seat's current preferred device-pixel size, so the launcher
  path reuses the same Desktop chooser/broker seam as the taskbar source picker.
  Farm evidence: `.90` focused
  `front_door_mesh_peer_rows_export_desktop_connect_action` passed; `.170`
  focused `front_door_peer_connect_request_routes_to_desktop_surface` passed;
  `.50` `cargo fmt -p mde-shell-egui --check` passed; warm `.90` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 37 tests.
  A later 2026-07-18 Front Door workflow-deep-link slice added an optional
  Workbench-plane target to workflow cards and wired the Mesh services card to
  the existing Workbench Provisioning plane through the same shell `Nav` seam
  Console already uses. Non-Workbench workload/service cards continue to open
  their real owner surfaces, so no fake service action was introduced. Farm
  evidence: `.50` `cargo fmt -p mde-shell-egui --check` passed; `.90` focused
  `workflow_search_items_expose_real_owner_cards_without_duplicating_apps`
  passed; `.170` focused
  `front_door_service_workflow_routes_to_workbench_provisioning_plane` passed.
  A later 2026-07-18 Front Door workflow-action slice added bounded selected-row
  quick actions for workflow cards that route to real Workbench Cloud/Fleet
  planes through a new `OpenWorkbenchPlane` request, keeping primary Open routed
  to each card's owner surface and avoiding fake start/stop controls without a
  lifecycle backend. The quick actions are also exposed to AccessKit with owner
  and target metadata. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 40 tests;
  current-source `.90` focused
  `cargo test -p mde-shell-egui front_door_workflow -- --nocapture` passed 3
  tests. A later 2026-07-18 Front Door filter-keyboard slice made the filter
  chips keyboard-operable through the shared focused click/Enter/Space
  activation predicate, paints the shared focus ring on focused chips, and adds
  Ctrl+Tab / Ctrl+Shift+Tab plus Alt+Right / Alt+Left cycling so keyboard-only
  users can traverse launcher filters without leaving the search workflow. Farm
  evidence: `.50` `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130`
  focused `cargo test -p mde-shell-egui front_door -- --nocapture` passed 41
  tests; current-source `.90` focused
  `front_door_filter_keyboard_traversal_cycles_filter_chips` passed. A later
  2026-07-18 Front Door rendered-proof slice added a headless egui raster proof
  for compact panel mode, expanded workflow mode, and degraded Mesh-source mode,
  writes PNG artifacts for all three states, checks the painted backend color
  stream is non-empty/varied, verifies the warning state paints a warm status
  fill, and extends the viewport guard to include rounded path and mesh shapes
  instead of only raw rect primitives. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui --check` passed; `.170` focused
  `front_door_rendered_proof_covers_panel_expanded_and_degraded_states` passed
  and wrote `front-door-compact-panel.png`,
  `front-door-expanded-workflows.png`, and `front-door-degraded-mesh.png`;
  BigBoy `.130` focused `cargo test -p mde-shell-egui front_door -- --nocapture`
  passed 42 tests. A later 2026-07-18 Front Door cloud-instance lifecycle slice
  added selected-row Start, Stop, and Reboot controls only for Mesh results with
  provable `cloud:instance:*` unit ids. Start publishes immediately; Stop and
  Reboot use the launcher arming pattern before dispatch. Shell handling writes
  the same `action/cloud/instance-*` typed `{"instance": ...}` bus body that the
  Explorer/OpenStack action lane already owns, and rejects non-instance mesh ids
  instead of minting fake service controls. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 46 tests,
  including lifecycle wire, arming, AccessKit, shell bus-write, and rendered
  Front Door proof coverage. Redundant narrower current-source lanes were
  stopped after BigBoy covered their assertions, per the no-filler-retest rule.
  A later 2026-07-18 Front Door Datacenter lifecycle slice added cached Fleet VM
  and container roster rows to the shared launcher search model, filters them
  through the Workloads/Services chips, routes primary Open to Workbench Fleet,
  and exposes selected-row Start/Stop/Restart actions only when the roster
  carries host, kind, name, and state metadata. Start publishes immediately;
  Stop and Restart use the launcher arming pattern before publishing the
  existing `action/services/lifecycle` directory contract with `{peer, kind,
  name, op}`. The projection reads cached Datacenter state only, so opening the
  launcher does not poll the Bus. Farm evidence: BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door_service_lifecycle -- --nocapture`
  passed 4 tests; `.90` focused
  `front_door_lifecycle_candidates_use_cached_vm_and_container_rosters` passed;
  `.170` focused
  `front_door_service_lifecycle_request_writes_directory_bus_action` passed;
  `.50` `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18
  Front Door peer-app lazy-load consumer slice added a shell-side
  `action/apps/peer-list` Bus request/reply cache, folds the daemon's
  installed-app reply into `FrontDoorPeerApp` rows, keeps the selected peer node
  stable once peer-app rows appear, and feeds those rows into the unified
  Front Door app list without depending on the `mackesd` crate or blocking on
  network peer discovery. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui -- --check` passed; `.90` focused
  `cargo test -p mde-shell-egui front_door_peer_apps -- --nocapture` passed 2
  tests; BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 62 tests,
  including the new shell Bus fold and selected peer-app context coverage.
  A follow-up 2026-07-18 Front Door peer-app launch slice changed peer-app
  primary actions from Desktop Connect to Launch, publishes a typed
  `action/apps/launch` Bus request with node/app_id/name, keeps Desktop Connect
  as a secondary action, and makes `mackesd` validate `app_id` against the
  peer's published `apps-installed.json` inventory before returning the existing
  desktop launch target. Farm evidence: `.50` fmt, `.90` `ipc::apps`, BigBoy
  `.130` focused `front_door_peer_app`, and BigBoy `.130` full `front_door`
  lanes passed. Actual remote process execution, parity retirement, and live
  `.15` Sunshine/Moonlight proof remain open.
  A later 2026-07-18 Front Door hover-polish slice replaced the expansion
  control's raw egui tooltip with a Front Door themed tooltip surface and added
  rendered text-color coverage so the launcher layout hover cannot regress into
  unreadable shared-shell popup text. A follow-up 2026-07-18 Front Door
  context-menu polish slice routed result row right-click menus through a
  Front Door visual scope, making popup/window/widget states use
  `Style::SURFACE`, `Style::SURFACE_HI`, `Style::TEXT`, and `Style::BORDER`
  before egui builds the native menu. Rendered row coverage proves Launch/Pin
  menu text paints with Front Door tokens and not raw black. Farm evidence:
  BigBoy `.130` focused
  `cargo test -p mde-shell-egui front_door_result_context_menu -- --nocapture`
  passed 2 tests; `.50` file-scoped `rustfmt --edition 2024 --config
  skip_children=true --check crates/desktop/mde-shell-egui/src/front_door.rs`
  passed. A later 2026-07-18 shell layout-profile
  tooltip slice replaced the lower-right Workstation/Tablet/Car layout control's
  raw egui hover text with a shell-themed tooltip surface using the current
  Style color roles, and added rendered paint coverage for tooltip text and
  surface colors so this platform control cannot regress into black-on-black
  popup text. Farm evidence: BigBoy `.130`
  `cargo test -p mde-shell-egui layout_profile_tooltip -- --nocapture` passed.
  A later 2026-07-18 Front Door
  narrow-expanded geometry slice made panel and expanded widths honor the
  margin-bounded viewport when a seat is narrower than the historical launcher
  minimum, and clamped expanded height to the visible screen so the launcher
  cannot paint sliced off the right or bottom edge on narrow displays. Farm
  evidence: `.50` `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130`
  focused `front_door_expanded_layout_uses_bounded_screen_geometry` passed.
  A later 2026-07-18 Front Door action-button readability/taxonomy slice made
  cramped selected-row action buttons paint icon-only with the Front Door
  themed hover label while preserving AccessKit button metadata. It also
  refreshed Start/Front Door assertions to use the shared
  `dock::LAUNCHER_GROUPS` labels (`Web`, `Developer Tools`) instead of retired
  `Web & Tools` test strings. Farm evidence: `.90`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130`
  `cargo test -p mde-shell-egui front_door -- --nocapture` passed 58 tests;
  BigBoy `.130` focused `start_tiles_use_the_shared_launcher_taxonomy_source`
  and `the_19_surfaces_are_grouped_into_shared_function_based_groups` passed.
  A later 2026-07-18 menu-bar Remote Sessions control slice found the shared
  top-right menu-bar/window-title-bar control already present, then tightened
  the shell-owned request drain: activating it now closes Front Door and
  compatibility Start Menu overlays before starting the visible minimize cue,
  keeps the previous active surface in place until the cue finishes, and routes
  through the existing `Nav` model to Remote Sessions. Added
  `menu_bar_remote_sessions_request_uses_shell_transition_and_closes_launchers`
  to exercise the public menu-bar request and shell transition path. Initial
  shell verification was blocked by an out-of-scope `mde-maps-location-egui`
  compile edge that is resolved in the current source; BigBoy `.130` then
	  passed the focused
	  `cargo test -p mde-shell-egui menu_bar_remote_sessions_request_uses_shell_transition_and_closes_launchers -- --nocapture`
	  gate. Farm evidence: `.50` `cargo fmt -p mde-shell-egui --check` passed before
	  the retest and BigBoy `.130` passed the shell transition test after
	  `LocationManager::primary_source` was present in `src/model.rs`. A later
	  2026-07-18 Desktop workspace chrome slice removed the Desktop/Remote
	  Sessions menu-bar mount and deleted the stale VDI Desktop menubar helper,
	  leaving the workspace as a bare session picker/remote desktop surface. The
	  empty chooser title now uses a centered backdrop status path, and the
	  top-right minimize-to-Remote-Sessions cue now paints a staggered card-shuffle
	  stack instead of a single shrinking rectangle. Farm evidence: BigBoy `.130`
	  focused
	  `desktop_workspace_body_does_not_mount_the_shared_menu_bar_button`,
	  `empty_roster_title_renders_near_the_workspace_center`,
	  `centered_status_places_the_empty_desktop_copy_in_the_workspace_center`,
	  and `menu_bar_minimize_effect_uses_staggered_card_shuffle_geometry` passed;
		  `.170` file-scoped
		  `rustfmt --edition 2024 --config skip_children=true --check` passed across
		  the touched shell GUI files. A follow-up 2026-07-18 Remote Sessions
		  tooltip polish slice routed chooser card-detail, Retry, and protocol-port
		  hovers through a chooser-local themed tooltip frame so Remote Sessions
		  hover copy cannot inherit raw egui popup colors. Farm evidence: BigBoy
		  `.130` focused
		  `chooser_hover_tooltip_uses_themed_text_and_surface` passed; `.90`
		  focused `the_filter_bar_and_grid_render_together` passed; `.50`
		  file-scoped
		  `rustfmt --edition 2024 --config skip_children=true --check` passed for
		  `chooser/render.rs` and `chooser/tests.rs`. A follow-up 2026-07-19
		  Remote Sessions popup polish slice routed the chooser filter/sort
		  ComboBox dropdowns and per-card right-click menu through a chooser-local
		  popup visual scope that resolves active light/dark `Style` palette roles
		  for surface, row states, disabled text, selection, and compact spacing
		  while preserving Connect / Pin / Retry / power / manual edit/remove
		  behavior. Farm evidence: BigBoy `.130` slot `chooser-popup-style`
		  `cargo test -p mde-shell-egui chooser_popup_surfaces_use_themed_text_and_compact_spacing -- --nocapture`
		  passed; `.90` slot `chooser-filter-render`
		  `cargo test -p mde-shell-egui the_filter_bar_and_grid_render_together -- --nocapture`
		  passed; `.50` slot `chooser-popup-fmt` file-scoped
		  `rustfmt --edition 2021 --check` passed for `chooser/render.rs` and
		  `chooser/tests.rs`. A later
		  2026-07-18 compatibility Start Menu panel slice bounded the legacy panel
	  width to the current screen, clamps excessive taskbar/rail reservations so
	  bad restored state cannot push the panel off-screen or produce negative
	  geometry, clips child panes to the bounded rect, and only paints the divider
	  when a right pane fits. Farm evidence: `.50`
	  `cargo fmt -p mde-shell-egui -- --check` passed; BigBoy `.130` focused
	  `cargo test -p mde-shell-egui start_menu_panel_geometry -- --nocapture`
	  passed 2 tests; `.90` focused
	  `start_taskbar_click_opens_front_door_and_survives_the_opening_click`
	  passed; BigBoy `.130` focused
	  `clean_super_tap_opens_front_door_without_the_start_button` passed. A
	  follow-up 2026-07-19 Front Door search-field polish slice replaced the
	  stock launcher `TextEdit` presentation with a Front Door-owned framed
	  search primitive: themed surface and border paint, compact inset, larger
	  search text, dim themed hint text, and the shared focus ring, while keeping
	  the same search query state and input AccessKit node. Farm evidence:
	  BigBoy `.130` slot `front-door-search-polish`
	  `cargo test -p mde-shell-egui front_door_search_field_uses_themed_hint_and_text -- --nocapture`
	  passed; `.50` slot `front-door-search-fmt` file-scoped
	  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/front_door.rs`
	  passed; local `install-helpers/lint-style-leaks.sh` passed with 0 leaks.
	  A follow-up 2026-07-19 compatibility Start Menu search-field polish slice
	  moved the still-mounted legacy Start search field onto resolved Start/Menu
	  theme colors: framed surface and border, larger query/hint typography,
	  themed search and clear icons, and shared focus-ring paint for dark mode
	  and Windows-2000 light mode. Farm evidence: BigBoy `.130` slot
	  `start-search-polish-test`
	  `cargo test -p mde-shell-egui start_menu_search_field_uses_themed_hint_and_query_text -- --nocapture`
	  passed; `.90` slot `start-search-polish-fmt2` file-scoped
	  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/start_menu.rs`
	  passed; local `install-helpers/lint-style-leaks.sh` and scoped
	  `git diff --check` passed. Live `.15` Sunshine/Moonlight visual proof
	  remains open.
- Origin or merged source IDs: `docs/design/app-launcher-rethink.md` APPLAUNCH,
  `docs/design/search-omnibox.md` Front Door/full omnibox slice,
  `docs/review/PLATFORM-REVIEW-2026-07-10.md` `shell-ux-2`, `shell-ux-3`,
  `shell-ux-8`, Start-menu portions of WL-UX-001 evidence, and shell
  Front Door search residuals currently referenced by WL-FUNC-005.

## Performance

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
  A later 2026-07-17 `.15` live-seat incident showed Google could leave the
  Browser interface unusable while the installed root-owned shell/CEF/mackesd
  processes continued running and libinput reported input-event lag. The static
  Browser page repaint path now uses a low-rate 250ms helper heartbeat for
  settled pages while preserving the fast 16ms cadence for loading, audible, or
  media pages; farm `.50` fmt and BigBoy `.130` focused `repaint_heartbeat`
  coverage passed. A follow-up same-day Google hardening pass added a per-tab
  fast-loading repaint budget: non-media pages that keep Chromium's loading bit
  set past the short first-paint grace now fall back to the 250ms helper
  heartbeat while active media remains on the 16ms cadence; farm `.50` fmt,
  BigBoy `.130` focused
  `long_loading_static_browser_page_drops_to_low_rate_heartbeat`, and warmed
  `repaint_heartbeat` coverage passed. A BigBoy `.130` Fedora 44 full RPM cut
  from cleaned HEAD
  `b9f84954` passed size guards and was staged on `.15` at
  `/home/mm/browser-f44-live-proof-b9f84954/` with sha256
  `db8ddcda749043dec5acd45c2daba953914750347a481dac94ac51f1c655016c` for
  `magic-mesh` and
  `18d91866730b0967e6a82b62ebcda82532f068e5c34851a7ae7b5c8fd97572db` for
  `magic-mesh-browser`; non-root `rpm -qp` on `.15` confirmed both packages as
  `12.0.0-1.x86_64`. The follow-up Google hardening commit `f9713f6f` was
  pushed, then BigBoy `.130` built Fedora 44 split RPMs in slot
  `browser-google-repaint-rpm`; both size guards passed (base 70.1 MiB,
  Browser 39.1 MiB). The packages were staged on `.15` at
  `/home/mm/browser-f44-live-proof-f9713f6f/` with sha256
  `e90f06d8234d90605c14146e375b077a7c70c95b62a978880fd85ab9c530449b` for
  `magic-mesh` and
  `1f6e46546f18b7ee3216e21425efe6608bb544c7a1c629ab2d48a23945054aa4` for
  `magic-mesh-browser`; non-root `rpm -qp` on `.15` confirmed both packages as
  `12.0.0-1.x86_64`. The same packages were installed on `.15` after a clean
  `rpm -Uvh --test --replacepkgs --force --nosignature`; `rpm -V
  magic-mesh magic-mesh-browser` returned clean, the shell restart used the
  documented restart-then-start tty handoff recovery, and the active service came
  back as `MainPID=671666`, `NRestarts=0`, start timestamp
  `2026-07-17 13:13:18 EDT`. The installed and running
  `/usr/bin/mde-shell-egui` hash matched the staged payload
  `cccd3f7905d48172abe3e2e412bee6414434c0b63852d0cc8261886e2fda1961`, and
  the installed CEF display/input verifier passed with process cleanup. A
  follow-up `.15` operator repro showed Google could still leave the Browser
  unusable when the helper stayed in loading state before first paint; non-root
  `.15` inspection confirmed the installed RPMs and root-owned
  `mde-shell-egui` process were still active, but root journal/proc inspection
  remained unavailable through `mm` sudo. The Browser loading heartbeat now
  keeps the low-rate 250ms helper wake alive after the fast-load grace even when
  no texture has been uploaded yet; farm `.50` fmt and BigBoy `.130` focused
  `long_loading_page_without_first_frame_keeps_low_rate_heartbeat` coverage
  passed. A BigBoy `.130` Fedora 44 split RPM cut from commit `61dcbae5` in
  slot `browser-google-prepaint-rpm` passed both size guards (base 70.1 MiB,
  Browser 39.1 MiB). The packages were staged on `.15` at
  `/home/mm/browser-f44-live-proof-61dcbae5/` with sha256
  `350f3559bce1b775622e068a8c2242f957a0cd93399f7f8f3b1e3b6a7d486030` for
  `magic-mesh` and
  `74709d34f041ef5dd994306d19ca4bdb47d3333bbff5ee60f39409ac7373bb1a` for
  `magic-mesh-browser`; non-root `rpm -qp` on `.15` confirmed both packages as
  `12.0.0-1.x86_64`. A 2026-07-18 `.15` recovery pass confirmed
  `sudo -n true` succeeds for `mm`, quarantined stale Browser session restore and
  send-tab replay state from `/root/.local/share/mde/browser-session-sync`,
  `/mnt/mesh-storage/browser-session-sync/Basement-Test-Workstation`, and
  `/run/mde-bus/action/browser/session-sync`, restarted
  `mde-shell-egui.service` to `ActiveState=active`, `SubState=running`,
  `NRestarts=0`, and passed the installed CEF/Servo display/input verifier with
  Browser helper cleanup. BigBoy `.130` then built the follow-up Fedora 44 split
  RPMs with payload size guards passing (base 72.8 MiB, Browser 39.0 MiB). The
  packages were staged on `.15` at
  `/home/mm/browser-f44-live-proof-20260718-022147/` with sha256
  `fde1f7e072e0e125488d30dbae9743647b25cf1cdffc8146cc454b8f32bee567` for
  `magic-mesh` and
  `5445248561e901338306b32f3fe2cc34c93e79528642fc1b402f109f9c514cdb` for
  `magic-mesh-browser`, installed after a clean same-version RPM transaction
  test, and restarted the seat to `MainPID=1890763`, `NRestarts=0`, timestamp
  `2026-07-18 02:22:20 EDT`. The running shell executable hash matched the
  installed payload, and the installed Browser verifier passed CEF and Servo
  display/input plus helper cleanup. No live screenshot proof has been claimed
  yet because the KMS capture attempt failed at ffmpeg format negotiation.
  A later 2026-07-17 repaint-budget pass narrowed active Browser media wakeups:
  audible tabs and media with unknown play state still keep the fast repaint
  cadence, while media explicitly reported as paused now drops to the existing
  low-rate helper heartbeat. Farm evidence: BigBoy `.130` focused
  `active_browser_media_with_unknown_play_state_keeps_fast_heartbeat`,
  `paused_active_browser_media_page_uses_low_rate_heartbeat`, and
  `cargo fmt -p mde-shell-egui --check` passed.
  A follow-up 2026-07-19 Browser hot-path slice removed the intermediate
  open-tab host `Vec` from `update_site_data_from_tabs`, preserving the
  site-data behavior while streaming owned host iterators directly into
  `SiteDataManager`; it also strengthened coverage for the existing Browser
  frame-retention and session-sync hot-path fixes by proving the retained
  `ColorImage` is shared with the texture upload via `Arc`, and that the
  per-frame session snapshot catch-all is throttled off the vblank path while
  unchanged snapshot bodies remain de-duped. Farm evidence: BigBoy `.130` slot
  `browser-hot-path`
  `cargo test -p mde-shell-egui browser_hot_path -- --nocapture` passed 3
  focused tests; `.50` slot `browser-hot-path-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-shell-egui/src/web/mod.rs` and
  `crates/desktop/mde-shell-egui/src/web/site_data.rs`. A follow-up
  2026-07-19 Browser resource-audit hot-path slice added a sequence-filtered
  `WebSession::recent_resource_requests_after` snapshot for poll paths that only
  need newly observed resource rows, then routed the shell mixed-content audit
  loop through that watermark so unchanged active tabs do not clone the full
  bounded resource history every Browser frame. Farm evidence: `.90` slot
  `browser-resource-client-test`
  `cargo test -p mde-web-preview-client
  recent_resource_requests_after_returns_only_newer_rows -- --nocapture` passed;
  BigBoy `.130` slot `browser-resource-hotpath-test`
  `cargo test -p mde-shell-egui
  resource_audit_hot_path_uses_sequence_watermark_for_new_rows -- --nocapture`
  passed; `.50` slot `browser-resource-session-fmt` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` passed for
  `crates/desktop/mde-web-preview-client/src/session.rs`; local `git diff
  --check` passed for the Browser files touched by this slice.
  A follow-up 2026-07-19 Browser multi-tab scheduler slice capped quiet inactive
  helper polling to two due background tabs per Browser panel frame, staggering
  large tab sets so they cannot all drain helper IPC in the same render pass
  while known playing background media still bypasses the quiet cap. Farm
  evidence: BigBoy `.130` slot `browser-bg-poll-cap` focused
  `cargo test -p mde-shell-egui background -- --nocapture` passed 6
  background/media tests, and the same warmed slot passed
  `cargo test -p mde-shell-egui
  many_due_inactive_browser_tabs_are_staggered_across_panel_frames -- --nocapture`.
  A follow-up 2026-07-19 Browser omnibox hot-path slice made the security/site-info
  resource snapshot lazy, so closed toolbar frames no longer clone the active
  tab's bounded resource history just to draw the security icon; the snapshot is
  now taken only when the site-info popup renders. Farm evidence: BigBoy `.130`
  slot `browser-omnibox-lazy`
  `cargo test -p mde-shell-egui
  omnibox_security_button_defers_resource_snapshot_until_popup_is_open --
  --nocapture` passed, the same warmed slot passed
  `cargo test -p mde-shell-egui
  site_info_panel_opens_from_the_security_chip_and_renders_without_panicking --
  --nocapture`, and `.50` slot `browser-omnibox-lazy-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs`.
  A follow-up 2026-07-19 Browser tab-strip favicon hot-path slice removed the
  frame-wide `Vec<Option<TextureHandle>>` allocation from both horizontal and
  vertical tab strips. Favicons now resolve on demand per rendered tab through
  `tab_favicon_texture_at`, while preserving the existing per-tab decode/cache
  behavior. Farm evidence: BigBoy `.130` slot `browser-favicon-demand`
  `cargo test -p mde-shell-egui
  tab_strip_favicon_resolution_is_on_demand_per_tab -- --nocapture` passed; `.50`
  slot `browser-favicon-demand-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs` and
  `crates/desktop/mde-shell-egui/src/web/mod.rs`; local `git diff --check`
  passed for the touched Browser files.
  A follow-up 2026-07-19 Browser resource-audit fast-check slice added
  `WebSession::has_recent_resource_requests_after`, letting the shell read the
  newest monotonic resource sequence before scanning/cloning the bounded
  resource-history window. Unchanged pages with already-audited resource rows
  now skip the mixed-content audit scan on panel frames. Farm evidence: `.90`
  slot `browser-resource-fastcheck-client`
  `cargo test -p mde-web-preview-client
  recent_resource_requests_after_returns_only_newer_rows -- --nocapture` passed;
  BigBoy `.130` slot `browser-resource-fastcheck-shell`
  `cargo test -p mde-shell-egui
  resource_audit_hot_path_uses_sequence_watermark_for_new_rows -- --nocapture`
  passed; `.50` slot `browser-resource-fastcheck-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-web-preview-client/src/session.rs` and
  `crates/desktop/mde-shell-egui/src/web/mod.rs`; local `git diff --check`
  passed for the touched Browser files.
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
