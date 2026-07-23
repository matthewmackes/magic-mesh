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

## Current Snapshot - 2026-07-22 takeover

- **10 active epics:** 7 `Remaining`, 3 `Blocked`; no `Needs clarification`.
- **P0:** WL-SEC-005 (final integrated gate), WL-SEC-006 (stop replicating
  Nebula private keys), WL-SEC-007 (authenticate privileged shared-Bus
  mutations), WL-ARCH-007 (authorization mint + direct lifecycle proof), and
  WL-FUNC-011 (blocked on real media/LLM resources).
- **In flight:** WL-BUILD-004 current-workspace coverage proof, WL-FUNC-012 live
  map feeds, WL-UX-006 Construct, and WL-UX-007 Car.
- **Externally blocked:** WL-RUN-003 needs a second lighthouse plus the operator's
  DigitalOcean credential; WL-FUNC-011 needs a real second media peer/SIP path
  plus an operator-sealed DigitalOcean model key; WL-SEC-006 needs a controlled
  live Nebula identity rotation/reconnect/prune drill.
- **Archived by this takeover:** WL-DOC-004, WL-FUNC-013, and WL-RUN-008 in
  `docs/worklist-archive/2026-07-22-platform-takeover.md`.

The reconciliation and operator-decision sections below are dated historical
context. Their old counts and execution suggestions do not supersede this
snapshot or the live epic records.

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

**Update 2026-07-20:** original 43 drained to 3 active — **WL-ARCH-001**
(CODE-COMPLETE; OpenStack removed + OpenTofu/Ansible/libvirt backend + iac/ workspace;
only Phase D live smoke, operator/hardware-gated), **WL-FUNC-011** (PARITY-COMPLETE;
full Communications stack + all 6 modes live; only the one-big cutover, operator-gated),
**WL-RUN-003** (held). New epic **WL-ARCH-006** (Workloads cockpit) added below as
WL-ARCH-001's surface successor (21-unit farm fan-out; **CODE-COMPLETE 2026-07-20 — all
21 units landed + workspace-green + pushed `bae119e6`; only live-seat smoke + mirror
rich-payload decode remain**).

**Update 2026-07-22 (operator: remove live-seat blocks; finish OpenStack removal):**
two closures + two clarifications.
- **WL-ARCH-001 → DONE + archived**
  (`docs/worklist-archive/2026-07-22-live-block-removal.md`): the OpenStack-removal
  live-apply block is removed; substantiated by `tofu validate` on `infra/tofu/cloud/` =
  valid and `ansible-playbook --syntax-check` on `site.yml` = clean. The live
  `MDE_CLOUD_APPLY=1` libvirt provision is now an optional operator spot-check.
- **WL-ARCH-006 → DONE + archived** (same file): the sole remaining gate was the
  live-seat `.15` provision→destroy smoke; removed per directive. Code-complete +
  `cargo build --workspace` green.
- **WL-FUNC-011** stays Blocked but its criterion-12 live-seat visual signoff is no
  longer a gate (the cutover shell is already deployed live + stable on `.15`); it
  remains blocked only on criterion 8 (real WebRTC/SIP call frames — live call infra + a
  2nd peer) and criterion 9 (DO LLM with an operator-sealed key) — neither is a seat.
- **WL-RUN-003** stays Blocked and is explicitly OUT of the live-seat directive's scope:
  its gate is a live cloud lighthouse fleet + a DigitalOcean API token (an operator-held
  secret), not a seat, and there is no build-time validation analog to substantiate a
  real add/retire against live etcd.
- Active count: 3 → **WL-RUN-003** (Blocked), **WL-FUNC-011** (Blocked),
  **WL-FUNC-012** (Remaining), **WL-FUNC-013** (Remaining, in progress).

**Fold-in 2026-07-22 (operator 50-Q survey: Apple HIG standard; two interfaces):**
ADR-0006 + `AI_GOVERNANCE.md` §4 amended — the design standard for the full
platform is Apple's HIG applied as principles; the platform has exactly two
interfaces, **Construct** and **Car**, with the single authority doc
`docs/design/platform-interfaces.md`. Nineteen interface-paradigm design docs
retired to `docs/design-archive/`. New epics **WL-UX-006** (Construct) +
**WL-UX-007** (Car) registered below. Dispositions: **WL-UX-001**
(Win10-taskbar live proof) is **superseded-retired** — the chrome it would
prove is scheduled for deletion at the WL-UX-006 cutover; **WL-UX-005**
(launcher overhaul) **folds into WL-UX-006** — its shipped Front Door engine
survives as Spotlight (reskin-only lock), its remaining Start-Menu-dedup
acceptance is moot (Start Menu already deleted, `115709a9`), and its
peer-app remote-exec remainder transfers to WL-UX-006's springboard scope.
Active count: 6 → adds **WL-UX-006** (Remaining), **WL-UX-007** (Remaining).

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

### WL-SEC-005 - Constrain root cloud-worker filesystem keys to owned state roots

- Status: Remaining
- Progress (2026-07-23): strict single-component validation now guards desired,
  image, version, container, placement, and lifecycle target sinks; sinks also
  reserve room for `.json` and `.container` suffixes before directory/backend
  I/O. The focused cloud suite passed 113/113 on farm `.170`, including the
  oversized desired-write, delete, container-staging, and lifecycle-delete
  regressions. Closure is waiting only on the final integrated format/diff
  gate while adjacent transport and overlay edits settle.
- Priority: P0
- Complexity: Medium
- Problem: Unauthenticated local Bus callers can supply absolute or traversing
  node, workload, image, version, and container identifiers to a root daemon,
  allowing cloud verbs to write or remove JSON outside the cloud state root.
- Required outcome: Every identifier used as a filesystem component is
  validated at the storage sink, and hostile requests cannot create, replace,
  or remove files outside daemon-owned state roots.
- Scope: `mackesd` desired-state, image, and container cloud verbs; strict
  component validation and hostile handler regressions. Bus-wide authentication
  and capability policy remain outside this focused containment item.
- Relevant files/components: `crates/mesh/mackesd/src/workers/cloud/path_key.rs`,
  `reconcile.rs`, `verbs/desired.rs`, `verbs/image.rs`, `verbs/container.rs`.
- Acceptance criteria: Absolute paths, separators, dot components, blank keys,
  and oversized keys fail before I/O; outside-sentinel write/delete regressions
  pass; ordinary hostnames and workload names continue to round-trip.
- Verification method: Focused `mackesd` cloud-worker tests on the build farm,
  plus `cargo fmt --all -- --check` and `git diff --check`.
- Origin or merged source IDs: 2026-07-22 Codex platform takeover security audit.

### WL-SEC-006 - Keep Nebula private keys local to their owning node

- Status: Blocked
- Progress (2026-07-23): code and hostile fixtures now meet the local-key design.
  Joining nodes generate their key locally; the signer consumes only the strict
  requester public key and verifies the returned certificate identity before an
  atomic swap. Public replicated bundles deny secret fields; legacy secret-bearing
  bundles fail closed; lighthouse secret enrollment is TLS-only, redacted, and
  persisted mode 0600 with symlink-hostile atomic replacement. Epoch rotation
  preflights/stages exact peer identities and transactionally rolls back. BigBoy
  farm proof is green: `mackesd` and `mde-enroll` all-target checks plus 204 focused
  CA/enrollment/client/endpoint/supervisor tests. The farm and available live seat
  have neither `nebula` nor `nebula-cert`, so only the controlled live
  rotation/reconnect/old-root-prune acceptance drill remains blocked; the
  operator has now torn down all DigitalOcean lighthouses, so there is no live
  Nebula peer on which to run that drill.
- Priority: P0
- Complexity: Epic
- Problem: Any node able to read replicated enrollment bundles can obtain other
  nodes' Nebula private keys and impersonate them. A compromised shared tree can
  also replace a relay trust authority and its signatures together unless the
  enrollment-pinned authority is held outside that mutable bundle.
- Required outcome: Each joining node generates and retains its own Nebula private
  key, the CA signs only the submitted public key, and no peer, CA, or relay private
  key is written to replicated state. Authenticated enrollment pins the relay
  authority in a root-owned local trust file; steady-state bundle updates must
  match that pin. Every remaining secret-bearing local write is atomic, durable,
  and explicitly mode `0600`.
- Scope: Nebula CSR/sign backend and wire contract, network/file enrollment
  delivery, steady-state bundle schema, local trust/key persistence, migration,
  revocation, and rotation of already-issued fleet identities. Public
  certificates, lighthouse rosters, and signed relay advertisements may remain
  replicated.
- Relevant files/components: `crates/mesh/mackesd/src/nebula_enroll.rs`,
  `nebula_enroll_client.rs`, `ca/sign.rs`, `ca/bundle.rs`,
  `workers/nebula_supervisor.rs`, and the `NebulaCertBackend` implementations.
- Acceptance criteria: The signer consumes the requester's exact public key and
  never receives/creates its private half; serialized steady-state bundles contain
  no private-key fields or PEM; a hostile peer reading every replicated file
  cannot authenticate as another node; mutable authority substitution fails
  against the local pin; migrated nodes rotate and revoke the former shared
  identities without losing overlay reachability.
- Verification method: Hostile serialization/permission tests, requester-key
  certificate match proof, two-node enrollment fixture with filesystem inspection,
  authority-substitution negative tests, farm suites, and a controlled live
  rotation/reconnect drill.
- Origin or merged source IDs: 2026-07-22 Codex takeover review of WL-RUN-008 trust
  bootstrap and pre-existing enrollment persistence.

### WL-SEC-007 - Authenticate privileged shared-Bus mutation consumers

- Status: Remaining
- Progress (2026-07-23): the typed action worker now requires schema v1 and an
  exact-body, 30-second, durably single-use HMAC capability before service
  lifecycle or code-edit dispatch; its legacy directory bypass only refuses.
  Code-edit writes are descriptor-relative and reject in-root symlink escapes.
  Cloud, direct Podman/libvirt lifecycle, and remote host-control consumers now
  use the same fail-closed gate; consumer-side expiry is capped at 30 seconds and
  direct libvirt rejects future schemas. Farm evidence is green for action
  28/28, cloud 109/109, direct libvirt 51/51 (including the latest future-schema
  and overlong-capability regressions), Podman 30/30, and host-state 13/13
  hostile/functional tests; the shared `host_ops` partition is 47/47 and
  `dc_power` is 30/30 (including unsigned, tampered, replayed, and future-schema
  refusal before backend execution).
  The Datacenter responder now gates its full VM/IaC/storage mutation set before
  op-lock or backend calls; its focused farm suite is 75/75, including signed
  one-shot replay refusal. VPN mutation gating is in flight on the farm.
  Publisher tracing found no legitimate Podman or remote host-control shell path;
  the scheduler's old unsigned actuator emission was retired rather than granted
  autonomous mint authority (32/32 farm-green). The reachability audit then found
  the higher-risk production `/run/mde-bus` tranche: registered IPC responders
  still accept unauthenticated Tofu apply/destroy, host power/
  network/secret operations, package uninstall, job launch, fleet revision,
  Connect/firewall, VPN, and DDNS mutations. It also found several async workers
  defaulting to a root-private data directory instead of the production spool.
  Common IPC gating, the complete reachability inventory, legitimate publisher
  wiring, and the shared-spool cross-UID negative fixture remain open. Tofu
  apply/destroy is now capability-gated (14/14 farm tests), Fleet
  push/rollback/nudge is capability-gated (8/8), and Jobs launch is
  capability-gated (4/4); their read verbs remain open. The shared seam's PTY,
  mesh-mount, physical-storage, and virtual-storage hostile/replay tests are
  individually green across isolated farm runs; mde-files is 162/162 and shell
  storage is 25/25. The dead
  `action/apps/uninstall` root package-removal channel had no production
  publisher and has been deleted rather than grandfathered; the remaining apps
  responder suite is 18/18 farm-green, including the retired-uninstall
  no-dispatch regression.
- Priority: P0
- Complexity: Epic
- Problem: The production Bus spool is intentionally cross-UID writable, but
  several root-daemon consumers still treat possession of an `action/*` topic as
  authority. A local process can therefore forge administrative requests; the
  takeover reproduced unauthenticated service-lifecycle and code-edit dispatch,
  and found the direct Podman lifecycle worker accepting forged run/stop/remove
  requests including host bind mounts.
- Required outcome: Every runtime-reachable privileged Bus mutation verifies a
  versioned, exact-body, short-lived capability and durably consumes its nonce
  before any side effect, or retains an existing cryptographic authorization
  protocol proved equivalent. Missing credentials fail closed. Retired mutation
  consumers are deleted instead of preserved as unauthenticated compatibility
  paths; read-only queries and harmless refresh nudges remain usable without an
  arm token.
- Scope: Inventory all root `mackesd` mutation consumers and their shell/CLI or
  worker publishers, including `action/*` and older mutable `compute/*` lanes;
  first close code edit, service lifecycle, direct Podman/libvirt, host
  power/control, provisioning/migration, onboarding, federation, firewall, and
  CA or secret-changing paths. Reuse the Workloads HMAC credential, exact-body
  digest, 30-second expiry, and host-local spent-nonce ledger. Message transport
  secrecy and per-user RBAC remain out of scope.
- Relevant files/components: `crates/platform/mde-bus/src/persist.rs`,
  `packaging/systemd/mackesd.service`, `crates/mesh/mackesd/src/workers/action.rs`,
  `container.rs`, `vm_lifecycle.rs`, `host_state.rs`, `xcp_provision.rs`,
  `compute_provision.rs`, `compute_expose.rs`, `compute_migrate.rs`,
  `onboard_apply.rs`, `federation_enforcer.rs`, `cert_authority.rs`, the
  production responder registrations in `src/bin/mackesd/spawn.rs`, privileged
  responders under `src/ipc/` (`tofu`, `datacenter`, `host_ops`, `dc_power`,
  `apps`, `jobs`, `fleet`, `connect`, `vpn_gw`, and `ddns`), and the corresponding
  publishers under
  `crates/desktop/mde-shell-egui/src/`.
- Acceptance criteria: A checked inventory classifies every production
  shared-Bus consumer with privileged effects as read-only/nudge, independently
  authenticated, newly
  capability-gated, or deleted; unsigned, expired, body-tampered, replayed, and
  future-schema mutations produce no side effect; an authorized request executes
  exactly once; no legacy topic bypass reaches the same sink; authorization
  refusal audit records never copy attacker-controlled code or secret payloads.
- Verification method: Hostile per-consumer fixtures on the build farm, a shared
  `0777` Bus-spool integration test that writes as an unprivileged publisher,
  focused shell-to-daemon wire tests, workspace gates, and a live credentialed
  mutation plus unsigned negative probe when hardware is available.
- Origin or merged source IDs: 2026-07-22 Codex takeover review of the production
  shared-Bus trust boundary; corrective successor to WL-SEC-005's explicitly
  out-of-scope Bus-wide authentication policy.

## Build, Installation, And Deployment

### WL-BUILD-004 - Make the mandatory gate cover the governed repository

- Status: Remaining
- Progress (2026-07-23): the canonical gate now runs one hard policy suite shared
  with GitHub Actions, exercises planted-failure self-tests, and propagates policy
  failures. The stale preview job and deleted coverage exclusions were removed.
  Takeover re-audit rejected the earlier closure, however: it had proved only
  `cargo metadata`, never the newly broadened 80% denominator. A clean detached
  `d52258e4` BigBoy run measured the actual all-library denominator at **84.69%
  lines** after repairing only its scratch lockfile, establishing useful margin.
  It also reproduced two test-target failures and proved the checked-in/current
  `--locked` command was not reproducible before the in-flight dependency lock
  reconciliation. The farm gate now runs workspace clippy and every test lane
  with `--locked`, and its serial `mackesd` lane enables the same
  `async-services` superset as GitHub Actions. A fresh BigBoy run of the exact
  integrated-tree coverage command passed at **84.67% lines** (80% floor), with
  the serial `mackesd` lane green at 3,702 tests and `mde-term-egui` green at
  391 tests. Workspace clippy and format checks are green. The earlier full-gate
  red was farm disk exhaustion during linking, not a test failure; disposable
  BigBoy slots were cleaned and the affected lanes rerun successfully. A
  clean-checkout release replay on `38971459` measured 84.67% lines (598,995
  regions at 84.97%) and remains the final closure proof after the current-tree
  format fixes land.
- Priority: P1
- Complexity: Medium
- Problem: CI previously referenced retired packages and split policy checks
  across incomplete runners. Its replacement now names current packages, but the
  new all-library coverage denominator was closed without ever being measured or
  shown to satisfy the advertised hard 80% floor.
- Required outcome: One maintained policy suite runs identically in the farm gate
  and GitHub Actions, all commands are reproducible with the committed lockfile,
  and a fresh current-workspace `cargo llvm-cov` run establishes and passes the
  hard 80% line floor on an explicitly documented denominator.
- Scope: `.github/workflows/ci.yml`, `install-helpers/ci-gate.sh`, lockfile
  reproducibility, current-package coverage configuration, and honest baseline
  evidence. This item does not waive the hard coverage policy or hide live
  packages merely to recover the historical percentage.
- Relevant files/components: `.github/workflows/ci.yml`,
  `install-helpers/ci-gate.sh`, `Cargo.lock`, and the build-farm coverage lane.
- Acceptance criteria: CI references no retired packages; policy self-tests and
  real-tree lints fail the aggregate gate when planted failures fire; the exact
  checked-in coverage command runs from a clean checkout with `--locked` and
  reports at least 80% lines over its documented current-package denominator.
- Verification method: Farm policy suite plus a clean BigBoy
  `cargo llvm-cov --workspace --locked --features mackesd/async-services ...
  --fail-under-lines 80 --summary-only` run matching GitHub Actions, followed by
  YAML parse, ShellCheck, format, and diff checks.
- Origin or merged source IDs: 2026-07-22 Codex platform takeover build audit;
  reopened after the closure evidence was found not to measure the new coverage
  denominator.

## Core Architecture

> WL-ARCH-001 (Remove OpenStack; OpenTofu+Ansible IaC) and WL-ARCH-006 (Workloads
> cockpit) both closed **DONE 2026-07-22** and moved to
> `docs/worklist-archive/2026-07-22-live-block-removal.md` (operator directive:
> remove the live-seat / OpenStack-removal live-apply blocks; both code-complete +
> IaC-validated).

### WL-ARCH-007 - Repair Workloads cockpit E2E wire, placement, and authorization

- Status: Remaining
- Progress (2026-07-23): UI contract slice landed in the takeover tree. Set
  desired now publishes the worker's `{node,spec}` envelope; provision,
  configure, plan, destroy, lifecycle, and console requests carry explicit
  placement; blank placement emits nothing. The request envelope is explicitly
  schema-v1 and future versions fail closed. Daemon routing now refuses blank
  placement, armed-token nonces are durably single-use across restart, the global
  destroy path is retired, and target delete independently checks the typed name
  before retracting only that workload's desired doc. Farm proofs before the
  latest version/replay additions: 36/36 focused `iac::` and 95/95 focused cloud
  tests; an integrated rerun is pending while adjacent files settle. The
  production root/systemd request path now wraps Datacenter VM/IaC/storage
  mutations in the shared HMAC capability gate, with unsigned and replayed VM
  dispatch refused before backend validation. Remaining: the integrated
  contract rerun and direct libvirt lifecycle drill.
- Priority: P0
- Complexity: Epic
- Problem: The archived WL-ARCH-006 surface is mounted, but its Set desired UI
  publishes a bare workload spec while the worker expects an envelope; mutation
  flows omit reliable node placement and authorization, blank placement fans
  out to every node, and destroy is workspace-wide instead of target-scoped.
- Required outcome: Every Workloads action has a versioned request contract,
  explicit single-node placement, production-minted replay-resistant authority,
  and target-scoped lifecycle semantics from UI through worker and runner.
- Scope: Workloads shell UI, cloud Bus request/reply contracts, placement,
  mutation authorization, replay protection, and targeted destroy. The already
  removed OpenStack backend stays out of scope.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/iac/`,
  `crates/mesh/mackesd/src/workers/cloud/`, `infra/tofu/cloud/`.
- Acceptance criteria: UI-to-worker contract tests cover set, provision,
  configure, console, and destroy; no blank-placement broadcast occurs; tokens
  are mintable and single-use; destroying one workload leaves peers intact.
- Verification method: Cross-crate contract fixtures, hostile/replay tests,
  focused farm suites, and a direct libvirt-host lifecycle drill when available.
- Origin or merged source IDs: corrective successor to archived WL-ARCH-006;
  2026-07-22 Codex platform takeover audit.

## Runtime Reliability

### WL-RUN-003 - Lighthouse full/equal join and push-button add/retire

- Status: Blocked
- Progress (2026-07-23): CODE-COMPLETE per `docs/platform/DRAIN-RECONCILIATION-2026-07-19.md`,
  plus the locked smallest-DigitalOcean lighthouse profile in
  `docs/design/digitalocean-lighthouse-small.md`. Both DO cloud-init paths and
  `onboard spawn-lighthouse` now default to `s-1vcpu-512mb-10gb` and apply
  bounded service memory, emergency swap, journal caps, and a control-plane-only
  optional-service set. The media-lighthouse class remains explicitly separate.
  typed `lighthouse_add`/`lighthouse_retire` (`cli/node_admin.rs:164`/`:205`), etcd voter
  membership (`cli/join.rs` `add_self_as_voter_blocking`, no manual etcdctl), CA
  inheritance, quorum-preserving `drain_gate` (`lighthouse_lifecycle.rs`), and the
  `spawn_lighthouse_onboard` worker + shell flow all landed. BLOCKED-on: the live
  multi-lighthouse fleet add-retire-add cycle drill + DO provisioning creds — operator/
  live-infra gated, not autonomously drainable. NB (2026-07-22): the operator's
  "remove live-seat blocks" directive does NOT reach this epic — its gate is a live
  cloud lighthouse fleet + a DigitalOcean API token (a secret only the operator holds),
  not a live seat, and there is no build-time validation analog (unlike WL-ARCH-001's
  `tofu validate`) that could substantiate a real add/retire against live etcd. Stays
  Blocked on a rebuilt DO fleet: the operator has torn down all DigitalOcean
  lighthouses, and the add-retire-add drill still requires a valid DO
  credential plus a second live lighthouse.
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

- Status: Blocked
- Progress (2026-07-21): CUTOVER LANDED (origin/master a84017f1) — at the AUTONOMOUS
  CEILING. Phase-1 (56 parity Qs ruled 78408f3b, migration importer 4e0d5df0, retire the
  dead Kamailio/RTPengine VV stack aad4d511) + Phase-2 shell surface cutover (a84017f1)
  are merged: Surface::Chat/Voice/Editor retired into one Surface::Communications,
  Surface::ALL 20->17, mde-voice-egui crate deleted (~3,530 LOC). Surface::Files KEPT
  (Q26 — not retired). mde-chat/mde-editor-egui/mde-voice-hud kept for their non-surface
  consumers. Full stack live + §7-audited stub-free: mde-collab-types + mde-collab-core
  (property-tested convergence) + mackesd CollabWorker (state/collab/* + action/collab/*)
  + mde-collab-egui CommunicationsSurface mounted in the shell. All 6 parity modes
  present. Landed farm-green (cargo build --workspace rc=0) with the failing-set proven a
  subset of base (zero new reds). Parity ledger docs/platform/WL-FUNC-011-parity-ledger.md
  (519 rows, all 56 open-Qs resolved). REMAINING = Phase-3 live-acceptance gates that are
  NOT live-seat (per operator 2026-07-22 the live-seat blocks are removed): criterion 8
  (real WebRTC/SIP call frames — needs live call infra + a 2nd peer), criterion 9 (DO LLM
  with a sealed key — needs an operator-provisioned key via mde-seal). Criterion 12 (live
  visual signoff) is NO LONGER a block: the cutover shell is already deployed live on .15
  (boots drm:true, 12.1.0, stable, NRestarts=0), so its functional half is closed and the
  aesthetic signoff is now an optional operator glance, not a gate. 4 PRE-EXISTING
  (non-cutover) shell-test reds documented in docs/NEEDS-OPERATOR.md. Phase-3c follow-ups
  (editor CRDT/three-way-merge/review; call media plane) remain co-edit/hardware-gated.
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
   image release after full parity and reproducible farm/live workflow evidence;
   human review is informative only.
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
  12. Deterministic rendered screenshots and workflow fixtures cover every
      feature at supported desktop and narrow/tablet sizes; no incomplete,
      disabled, placeholder, or deferred behavior remains. Human visual review
      is informative only and is not a release gate.
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
  Final closure requires the parity ledger and reproducible render/workflow
  evidence; human visual review is informative only.
- Origin or merged source IDs: `NOTIFY-CHAT`, `EDITOR-1..12`,
  `EDITOR-LSP-1..3`, `EDITOR-COLLAB-1..3`, `EDTB-1..7`, `FILEMGR-*`,
  `TRANSFERS-*`, `E12-11`, `VOIP-GW-*`, Clipboard and alert-relay workstreams,
  operator text-editor survey, and operator 50-question Communications
  collaboration survey completed 2026-07-19.

### WL-FUNC-012 - Maps live-data overlays (zero-cost external feeds)

- Status: Remaining
- Progress (2026-07-23): all ten catalog feeds are implemented through
  typed latest-wins Bus snapshots and the Maps painter: USGS earthquakes, NWS
  alerts, NWS hourly route forecast, adsb.lol aircraft, GTFS-Realtime transit,
  Caltrans cameras, IEM NEXRAD radar, NCDOT TIMS state-511 traffic, and NIFC/FIRMS
  wildfire, and AirNow AQI. The overlays close their typed schemas, registered
  worker/spawn census, off-by-default layer toggles, attribution, projected pins,
  bounded payloads, and paused/fix-loss behavior. AirNow evidence is green for
  mesh types 2/2, Maps/model 5/5, worker 7/7, worker-role census 19/19, and
  full Maps 136/136; its missing sealed key remains an honest unconfigured
  state with no network request or fabricated fetch time. Keyed feeds must idle
  honestly until operator-sealed free credentials exist.
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

## User Interface And Experience

### WL-UX-006 - Construct interface (Apple-HIG-principled workstation shell)

- Status: Remaining
- Progress (2026-07-22): interrupted U10/U11/U23 work was recovered into the main
  tree: the persistent eight-page Springboard base, slim top status bar, shared
  Workbench `NavigationBar`, and shared Console/Workloads style tokens. Farm
  evidence: status bar 8/8, Workbench 16/16, Console 47/47; the repaired
  integrated shell run passed **1,706/1,706** tests with zero failures. The three
  salvaged dirty Claude worktrees were removed after zero-unique-commit
  verification. Remaining acceptance is deterministic render/pixel capture and
  live VDI behavior; human visual review is informative, not a gate.
- Priority: P1
- Complexity: Epic
- Problem: The workstation chrome is Win10-shaped (48px bottom taskbar + tray
  flyouts, `src/dock/mod.rs`) with an ephemeral search launcher and no home
  screen, after three chrome reversals in ten days; no single design standard
  governs the shell, and the operator's locked direction (ADR-0006: Apple HIG
  as principles, iPadOS structure + macOS pointer manners) has no
  implementation.
- Required outcome: Construct per `docs/design/platform-interfaces.md` Part I -
  persistent springboard home (pages = the 8 LAUNCHER_GROUPS, no dock, no
  widgets), slim top status bar, Control Center, Notification Center,
  Spotlight (Front Door engine, keyboard flow byte-identical), card app
  switcher with snapshot previews, shared
  NavigationBar/Toolbar/Sidebar/Sheet/Popover components adopted by all 17
  surfaces, scrim materials + HIG radii + zoom-from-tile motion, two-profile
  LayoutProfile (Construct + Car, Tablet folded via serde aliases), and the
  Win10 chrome DELETED at cutover (no legacy flag).
- Plan: `/root/.claude/plans/the-workstation-interface-should-cozy-minsky.md`
  (28-unit + 2-gate fan-out; main.rs serialization queue U25→U08→U09→U27→U29).
- Relevant files/components: `crates/desktop/mde-shell-egui/src/` (main.rs,
  dock/ [deleted at cutover], front_door.rs, new springboard.rs / status_bar.rs /
  control_center.rs / notification_center.rs / switcher.rs / surfaces.rs,
  curtain.rs, keyboard.rs, system/), `crates/shared/mde-egui/src/` (style.rs,
  motion.rs, fonts.rs, gestures.rs, new nav_chrome.rs / sheet.rs).
- Dependencies: WL-FUNC-012 shell-side hooks land before the cutover unit
  (same-crate serialization); curtain lock security behavior and the VDI
  full-native-resolution guarantee are sacred (zero logic diffs).
- Acceptance criteria: machine-captured screenshot/pixel proof on the `.15` DRM seat -
  springboard pages (all 8), status bar, Control Center, Notification Center,
  Spotlight, switcher with real snapshots, zoom transitions, VDI full-res with
  auto-hidden bar; post-cutover grep gate (zero taskbar identifiers in
  production code). Human visual review is informative only.
- Verification method: per-unit farm builds + targeted tests; two integration
  slots (`cargo build --workspace` + `cargo test --workspace --no-run` + full
  run + lint-style-leaks/doc-supersession/worklist) after the shared-API units
  and after cutover; live `.15` deploy with
  `--features drm,live-helper,live-vdi,media-mpv`.
- Origin or merged source IDs: operator 50-Q survey 2026-07-22 (ADR-0006);
  supersedes WL-UX-001 (retired); absorbs WL-UX-005 (launcher overhaul -
  Front Door survives as Spotlight; peer-app remote-exec remainder).

### WL-UX-007 - Car interface (CarPlay-principled vehicle mode)

- Status: Remaining
- Progress (2026-07-22): retained MG90 telemetry now expires after five seconds;
  stale/simulated readings re-dash, cannot drive motion policy, and keep their
  provenance/age honest. Vehicle Fuel/Odometer now share the same live gate.
  Farm `mde-maps-location-egui` suite passed 97/97. Settings now consumes
  `glance_clamp` to shorten the moving-car rail without hiding destinations from
  search/menus, while host-down power prompts consume `deferred_notice` and emit
  no action until stopped (Lock stays available). The focused Car policy suite
  passed 8/8 and the moving Settings paint/defer regression passed 1/1 on farm
  `.130`. Remaining is live MG90 + DRM-seat proof; human review is informative
  only.
- Priority: P1
- Complexity: Epic
- Problem: Car mode is a SYNC 3-styled 2x3 tile grid whose 7th tile wraps, with
  a driver instrument strip that goes STALE off the Maps surface (the vehicle
  fold ran only inside the `Surface::MapsLocation` render arm), no codified
  glanceability/driving-safety requirements, and a design doc
  (auto-mode-sync3.md) now superseded by the platform standard.
- Required outcome: Car per `docs/design/platform-interfaces.md` Part II -
  CarPlay-principled with the SYNC3 dark palette kept: Dashboard-cards home
  (persistent Nav-map/Media/glance cards + app strip), six apps (Nav, Media,
  Music [new], Comms [Phone merged], Vehicle, Settings; Airspace tile dropped),
  the left 1/3 instrument strip fresh on EVERY Car screen (per-frame fold,
  2 Hz self-throttle - fix landed with this fold-in), glance rules + soft
  in-motion limits above the MG90 speed threshold (no hard lockouts), one-tap
  toggle only (no auto-enter), always dark.
- Plan: same plan of record as WL-UX-006 (units U25-U28 + gate U32).
- Relevant files/components: `crates/desktop/mde-shell-egui/src/car_home.rs`,
  `src/main.rs` (car_instrument_strip, central_view car branch, car_keymap
  routing), `crates/desktop/mde-maps-location-egui/` (car_status.rs, model.rs
  vehicle fold), `crates/shared/mde-egui/src/style.rs` (SYNC3 tokens).
- Dependencies: `state/vehicle/<node>` MG90 mirror (Rolling Node epic) for the
  live drive proof and the in-motion speed signal; Music surface split from
  Media in the car roster.
- Acceptance criteria: deterministic live-test proof with the MG90 mirror online - dashboard cards
  live, instrument strip fresh on every Car screen, soft limits engage above
  threshold, one-tap toggle; honest sparse data (never fabricated readings);
  no human signoff gate.
- Verification method: per-unit farm builds + targeted tests (car_home,
  car_status, keymap); live MG90 drive verification
  (`ssh -p2222 root@172.20.0.25` publishes the mirror).
- Origin or merged source IDs: operator 50-Q survey 2026-07-22 (ADR-0006);
  supersedes auto-mode-sync3.md as Car design authority (palette tokens
  survive); stale-telemetry fix hoisted per survey Q33.

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
