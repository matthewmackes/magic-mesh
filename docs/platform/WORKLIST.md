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

## Current Snapshot - 2026-07-24 security closure

- **6 active epics:** 6 `Remaining`, 0 `Blocked`; no `Needs clarification`.
- **P0:** WL-SEC-006 (stop replicating Nebula private keys), WL-ARCH-007
  (authorization mint + direct lifecycle proof), and
  WL-FUNC-011 (optional real media/LLM evidence remains).
- **In flight:** WL-FUNC-012 live map feeds, WL-UX-006 Construct, and WL-UX-007
  Car. The 2026-07-23 thin-lighthouse policy is enforced in role pinning,
  onboarding, install profiles, directory discovery, DNS, workers, secret
  scope minting, and both media helpers; no new lighthouse may carry media or
  file-sharing duties.
- **Non-blocking external evidence:** WL-FUNC-011 still has optional real
  second-peer/SIP and sealed DigitalOcean model demonstrations; WL-SEC-006
  still has an optional controlled Nebula identity rotation/reconnect/prune
  demonstration. These proofs no longer block autonomous implementation or
  the active drain; missing resources remain explicitly recorded below.
- **Archived by this takeover:** WL-DOC-004, WL-FUNC-013, and WL-RUN-008 in
  `docs/worklist-archive/2026-07-22-platform-takeover.md`; WL-SEC-005 and
  WL-BUILD-004 are archived in `docs/worklist-archive/2026-07-23-thin-drain.md`;
  WL-SEC-007 is archived in `docs/worklist-archive/2026-07-24-sec007-closure.md`.

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

Proof classification (operator decision 2026-07-24): rendered screenshots,
physical input round-trips, live peer demonstrations, and sealed external-feed
exercises are evidence follow-ups, not blockers to autonomous implementation.
They may remain listed in an epic's acceptance/evidence section, but they must
not be the sole reason an epic carries `Blocked`; missing resources are named
honestly without stopping the drain.

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

### WL-SEC-006 - Keep Nebula private keys local to their owning node

- Status: Remaining
- Progress (2026-07-24 relay-authority refresh hardening): the Nebula supervisor
  now refuses replicated bundle refresh and lighthouse-roster reconciliation
  when the public relay authority does not match the root-local enrollment pin.
  The foreign-authority regression is covered by the focused BigBoy supervisor
  gate at 40/40; live rotation/reconnect evidence remains external.
- Progress (2026-07-24 enrollment framing hardening): the TLS enrollment parser
  now rejects unsupported `Transfer-Encoding` and ambiguous
  `Transfer-Encoding`/`Content-Length` combinations before request dispatch.
  The focused BigBoy endpoint gate is green at 18/18; live enrollment remains
  external.
- Progress (2026-07-24 HTTP header framing hardening): the TLS enrollment parser
  now caps the response header block at 16 KiB before scanning or allocating the
  enrollment body, rejecting oversized hostile headers fail-closed. The focused
  BigBoy endpoint gate is green at 21/21; live enrollment remains external.
- Progress (2026-07-24 Nebula refresh retry hardening): a failed bundle or
  blocklist refresh no longer advances the supervisor's watch markers, so a
  transiently partial or hostile update is retried on the next tick and then
  acknowledged only after successful materialization. The focused retry gate is
  green at 1/1 and the full supervisor module at 41/41; live rotation remains
  external.
- Progress (2026-07-23): code and hostile fixtures now meet the local-key design.
  Joining nodes generate their key locally; the signer consumes only the strict
  requester public key and verifies the returned certificate identity before an
  atomic swap. Public replicated bundles deny secret fields; legacy secret-bearing
  bundles fail closed; lighthouse secret enrollment is TLS-only, redacted, and
  persisted mode 0600 with symlink-hostile atomic replacement. Epoch rotation
  preflights/stages exact peer identities and transactionally rolls back. BigBoy
  farm proof is green: `mackesd` and `mde-enroll` all-target checks plus 204 focused
  CA/enrollment/client/endpoint/supervisor tests. The farm and available live seat
  have neither `nebula` nor `nebula-cert`, so the controlled live
  rotation/reconnect/old-root-prune demonstration remains unperformed; the
  operator has now torn down all DigitalOcean lighthouses, so there is no live
  Nebula peer on which to run it. A post-policy attempt to provision a fresh
  smallest-size DO lighthouse was refused by the API with HTTP 401, so no cloud
  state changed. This is a non-blocking evidence follow-up, not a reason to stop
  autonomous implementation.
- Progress (2026-07-24 DO access recheck): the configured `doctl` context still
  returns HTTP 401, and the node-local secret store has no `do-token`; no
  droplet or cloud state was created. A fresh operator token is still required
  before the optional live Nebula demonstration can run.
- Progress (2026-07-24 Eagle/.15 mesh recheck): Eagle is enrolled at
  `10.42.0.3` and `.15` at `10.42.0.8`; both `nebula` and `mackesd` are active,
  but handshakes to the existing lighthouse endpoints and between the two
  seats time out. The supplied DigitalOcean token was tested directly and
  remains rejected with HTTP 401, so the requested two new cloud lighthouses
  cannot be created yet and no cloud state was changed.
- Progress (2026-07-24 enrollment identity hardening): reusing an existing
  enrollment identity now validates both files with no-follow semantics and
  rejects arbitrary symlinks or private keys that are not owner-only `0600`.
  Hostile symlink/permission fixtures are included; the focused farm endpoint
  suite passes 16/16. Live Nebula rotation remains optional evidence.
- Progress (2026-07-24 DOM0 lighthouse recovery): the hermetic four-node
  `test-lighthouse-replace.sh` drill ran on XEN-HOME-SERVICES with the thin
  lighthouse RPM and passed 33/33. It proved found/join, Nebula overlay
  reachability, three-member etcd health, survivor behavior after lighthouse
  loss, stale-member removal, replacement-lighthouse enrollment, replacement
  quorum health, and the absence of FUSE/LizardFS mounts. This clears the
  lighthouse lifecycle proof in the controlled environment; production
  DigitalOcean provisioning remains optional external evidence pending a valid
  operator token.
- Progress (2026-07-24 supervisor stale-state hardening): leadership state now
  advances only after a successful promote/demote transition, so a failed role
  change remains pending for the next retry instead of being recorded as applied.
  The role marker is owner-checked, schema-bound, node-bound, and lease-checked;
  the focused supervisor farm suite is green at 34/34.
- Progress (2026-07-24 sealed-file race hardening): CA and Nebula sealed-file
  reads now consume the descriptor opened after no-follow validation, canonical
  generation switches are validated as owner-controlled, and atomic writes
  reject symlinked parent components before and after directory creation. The
  hostile sealed-file farm suite is green at 14/14; the optional live Nebula
  rotation/reconnect/prune demonstration remains external evidence.
- Progress (2026-07-24 CSR freshness hardening): Nebula enrollment now rejects
  CSRs older than five minutes or future-dated beyond five minutes before bearer
  consumption or signing. Stale and future-dated regressions are covered; the
  focused BigBoy enrollment gate is green at 45/45.
- Progress (2026-07-24 supervisor bootstrap hardening): lighthouse promotion now
  follows the authoritative non-expired lease even when the local role marker
  is missing, then creates or repairs that marker. The mirror puller retains the
  stricter marker-plus-lease contract. The focused BigBoy supervisor suite is
  green at 35/35, including clean-marker promotion and failed-demotion retry.
- Progress (2026-07-24 public-lighthouse installer onboarding): `mde-enroll` now
  carries the closed public roster `lighthouse1.ephemeral.team` through
  `lighthouse3.ephemeral.team`, keeps the pasted mesh token and its CA pin
  immutable, and permits only those roster names or an explicitly entered pinned
  IPv4 override. Unknown hostnames and missing pins fail closed before network
  access. The focused roster suite is green at 6/6 and the installer binaries
  pass the farm compile check; live DNS/DO provisioning remains external.
- Progress (2026-07-24 Nebula roster export hardening): public roster export now
  fails closed above 4,096 rows while retaining deterministic ordering, epoch
  de-duplication, and revoked-certificate filtering. The focused BigBoy roster
  suite is green at 7/7.
- Progress (2026-07-24 federation UI capability hardening): federation accept,
  revoke, and refuse-mint mutations now publish schema-versioned, root-scoped
  armed capability envelopes with validated action targets; pending offers
  visibly report remaining lifetime and fail closed to an expired/degraded state
  when the expiry or clock is unavailable. The focused BigBoy shell federation
  suite is green at 9/9.
- Progress (2026-07-24 federation status-boundary hardening): the runtime
  federation status mirror now bounds display fields and row counts and enforces
  the shared 64 KiB Bus body limit while retaining total counts and explicit
  truncation indicators. The focused BigBoy federation-enforcer suite is green
  at 7/7.
- Progress (2026-07-24 enrollment response-boundary hardening): the
  fingerprint-pinned enrollment client now caps complete HTTP responses at
  256 KiB before bundle parsing, using a one-byte-over-cap read to fail closed
  without unbounded buffering. The focused `.170` `nebula_enroll_client` suite
  is green at 12/12.
- Progress (2026-07-24 relay-authority pin hardening): root-local authority pins
  now require canonical lowercase hexadecimal, are created once with atomic
  no-overwrite semantics, and reject hostile symlink targets; replicated bundle
  reads use the no-follow sealed-file path. The focused BigBoy CA/authority
  suite is green at 16/16. Live Nebula rotation remains optional external
  evidence.
- Progress (2026-07-24 enrollment authority-binding hardening): when a
  lighthouse response carries a relay-authority private seed, the enrollment
  client now derives and verifies its public key against the authenticated
  bundle before persisting the local trust anchor. The focused BigBoy client
  suite is green at 13/13; live Nebula/credential proof remains external.
- Progress (2026-07-24 enrollment HTTP framing hardening): the TLS enrollment
  parser now rejects repeated `Content-Length` headers instead of allowing an
  ambiguous framing choice to reach the JSON handler, and enrollment responses
  carry `Cache-Control: no-store` because authenticated lighthouse responses can
  contain sensitive CA material. The focused BigBoy endpoint suite is green at
  17/17; live Nebula/lighthouse evidence remains external.
- Progress (2026-07-24 bearer grammar hardening): Nebula join-token validation
  now rejects nonterminal `=` padding inside bearer values and accepts padding
  only at the terminal boundary, preventing malformed bearer text from reaching
  enrollment. The focused `.50` enrollment suite is green at 46/46; live
  Nebula/lighthouse evidence remains external.
- Progress (2026-07-24 backup-armor boundary hardening): CA backup dearmor now
  rejects an incomplete export without its exact END delimiter, while the
  existing validated transactional restore remains covered. The focused `.50`
  CA backup gate is green at 29/29; live CA restore evidence remains external.
- Progress (2026-07-24 enrollment client response framing): the pinned TLS
  enrollment client now validates the HTTP/1.1 status line, header names, exact
  `Content-Length`, duplicate-length ambiguity, and unsupported transfer
  coding before JSON parsing. Four hostile framing regressions are covered; the
  focused BigBoy client gate is green at 17/17. Live Nebula/lighthouse evidence
  remains external.
- Progress (2026-07-24 pending-CSR key-boundary hardening): peer enrollment now
  validates the submitted Nebula X25519 public-key PEM before bearer
  authorization, signer scratch creation, certificate-row insertion, or bundle
  writes. The hostile malformed-key regression is included in the focused
  BigBoy enrollment gate at 93/93; live Nebula evidence remains external.
- Progress (2026-07-24 requester-key staging boundary): network enrollment now
  validates every config-root component as a real directory before creating the
  requester staging tree, and revalidates the fresh staging directory before
  invoking `nebula-cert`; symlinked, non-directory, and parent-component paths
  fail before any key write. The focused BigBoy enrollment-client gate is green
  at 20/20; live Nebula/lighthouse evidence remains external.
- Progress (2026-07-24 identity-materialization path boundary): Nebula config and
  identity roots now use no-follow, component-by-component directory creation
  before CA, certificate, or private-key materialization. Four hostile
  symlink/non-directory regressions are included in the BigBoy supervisor gate
  at 39/39; live Nebula rotation remains external.
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

## Build, Installation, And Deployment

## Core Architecture

> WL-ARCH-001 (Remove OpenStack; OpenTofu+Ansible IaC) and WL-ARCH-006 (Workloads
> cockpit) both closed **DONE 2026-07-22** and moved to
> `docs/worklist-archive/2026-07-22-live-block-removal.md` (operator directive:
> remove the live-seat / OpenStack-removal live-apply blocks; both code-complete +
> IaC-validated).

### WL-ARCH-007 - Repair Workloads cockpit E2E wire, placement, and authorization

- Status: Remaining
- Progress (2026-07-24 image-reference boundary hardening): image-build now
  validates caller-controlled OCI references before capability replay or builder
  dispatch, rejecting option-shaped, malformed, whitespace/control, path, and
  invalid-digest values while retaining the literal `--` argv boundary. The
  focused BigBoy image suite is green at 36/36; live backend evidence remains
  external.
- Progress (2026-07-24 container lifecycle ownership hardening): Restart, Logs,
  and Destroy now require the target to be a declared `service_container` on
  the requested placement before authorization/replay or systemd execution.
  The focused BigBoy lifecycle gate is green at 10/10; live Podman/systemd
  success evidence remains external.
- Progress (2026-07-24 console identity hardening): `console-attach` now rejects
  requests whose `name` and lifecycle `instance` identify different workloads,
  before dispatching to console resolution. The focused cloud-console farm gate
  is green at 11/11; live backend evidence remains external.
- Progress (2026-07-24 Workloads mirror freshness hardening): the shell now
  treats a missing, stale-after-three-heartbeats, or far-future `state/cloud`
  timestamp as non-current; stale nodes cannot advertise live apply, and the
  Status, placement, menu, and workload-metric surfaces label the retained data
  honestly. The focused IAC farm gate is green at 44/44 plus an isolated stale-
  capability regression at 1/1; live Podman/libvirt evidence remains external.
- Progress (2026-07-23): UI contract slice landed in the takeover tree. Set
  desired now publishes the worker's `{node,spec}` envelope; provision,
  configure, plan, destroy, lifecycle, and console requests carry explicit
  placement; blank placement emits nothing. The request envelope is explicitly
  schema-v1 and future versions fail closed. Daemon routing now refuses blank
  placement, armed-token nonces are durably single-use across restart, the global
  destroy path is retired, and target delete independently checks the typed name
  before retracting only that workload's desired doc. The final integrated
  BigBoy `mackesd --lib` gate passed 3,872 with 0 failures and 1 ignored;
  focused cloud security and direct lifecycle suites passed 112/112 and 53/53
  respectively. The
  production root/systemd request path now wraps Datacenter VM/IaC/storage
  mutations in the shared HMAC capability gate. Remaining: a direct libvirt
  lifecycle drill against an available backend; the farm currently has no
  `virsh`/libvirt backend, so the 51-test seam proof is recorded but not
  promoted to live-host evidence. The lifecycle seam now also has a complete
  deterministic create/start/pause/resume/stop/destroy round-trip with asserted
  state and call order; the focused farm proof is 53/53. This strengthens the
  headless contract but does not change the direct-libvirt blocker. Chooser
  lifecycle controls now fail closed when `local-kvm` capability evidence is
  missing or unknown, including the right-click VM power menu; the focused
  no-libvirt/context-menu proof is 1/1. The Workloads state and Provision UI
  now also refuse to arm or publish live apply for a missing, stale, plan-only,
  or otherwise unarmed selected node; focused IAC coverage is 39/39 locally,
  including the plan-only negative path. The current selected-node arm and
  pre-authorization recheck tranche is farm-green at 20/20. The `.15` host now
  has the Fedora modular libvirt/QEMU stack, active default network,
  active/autostart `mde-vms` pool, and active Podman socket. A real exact-body
  HMAC `vm-create` request crossed the Bus and reached `virsh define`; the
  request failed honestly because firmware has VMX disabled and `/dev/kvm` is
  absent. Thus the host plumbing and authorized backend path are live; the
  direct KVM lifecycle demonstration is a non-blocking firmware/physical-host
  evidence follow-up rather than a code or routing blocker.
  The latest cloud contract rerun is green at 116/116 after adding explicit
  schema-v1 fixtures for the container/image mutation subverbs; missing and
  future mutation schemas fail before placement/backend dispatch. The direct
  lifecycle seam remains 53/53 and the live KVM blocker is unchanged. KVM
  health now recognizes socket-activated Fedora modular libvirt providers
  (`virtqemud.socket`, `virtnetworkd.socket`, `virtstoraged.socket`) without
  changing the stable service IDs or conflating service availability with
  `/dev/kvm`; the focused catalog/health proof is 19/19. The datacenter
  responder's structural mutation boundary now has a farm-green 119/119
  selected-test gate proving unsigned `vm-create`, `lighthouse-create`, and
  `genesis-write` requests are refused before any Tofu file is written. The
  lifecycle seam also has a target-scoped destroy regression: the selected
  domain and its storage flag are removed without touching unrelated domains;
  the focused farm proof is 54/54. Cloud dispatch now retains malformed-body
  state and refuses invalid JSON before both read and mutation handlers while
  preserving valid legacy `{}` reads; its focused farm suite is 57/57.
  The per-node desired-state store now rejects symlinked directory components,
  ignores symlinked documents on read, refuses symlink removal, and atomically
  replaces document leaves without following a planted link. Its corrected
  focused hostile-filesystem suite is green at 12/12, including preservation of
  the pre-existing invalid-name error contract. The latest integrated BigBoy
  `mackesd --lib --features async-services` gate is green at 3,896 passed,
  0 failed, and 1 ignored. Container workloads now have a complete bounded
  lifecycle path: the UI's Restart, Logs, and Destroy controls emit the real
  `container-restart`, `container-logs`, and `container-destroy` verbs with the
  row's placement and identity; Destroy remains typed-confirmation gated. The
  worker validates path-safe unit stems, enforces placement/schema/auth/replay
  boundaries, invokes literal-argv `systemctl`/`journalctl` commands through the
  runner, retracts only the selected desired entry after a successful destroy,
  and reports unavailable or failed backends honestly. Focused farm evidence is
  UI IAC 43/43 and cloud verbs 62/62, both with zero failures. Remaining live
  evidence is limited to exercising a real Podman/systemd unit on an available
  placement host; no live success is claimed by the fixtures.
- Progress (2026-07-24 console boundary hardening): the console verb now rejects
  path-like, whitespace, overlong, or leading-dash workload targets before
  authorization or `virsh`; malformed hosts and port zero cannot become console
  URIs; loopback and wildcard bindings are gated until a retained VDI broker can
  relay them. The focused BigBoy console suite is green at 10/10.
- Progress (2026-07-24 image-builder argv hardening): image-build now places an
  end-of-options separator before the caller-controlled image reference, so a
  leading-dash reference cannot inject another builder flag. The focused BigBoy
  async-services image suite is green at 11/11.
- Progress (2026-07-24 container lifecycle output hardening): rootless Quadlet
  restart/log/destroy handling now bounds backend output to 64 KiB with
  UTF-8-safe truncation before it enters Bus replies or audit records. The
  focused `.90` container lifecycle suite is green at 6/6.
- Progress (2026-07-24 direct-handler placement hardening): the cloud worker now
  re-checks placement at its direct handler boundary before capability replay
  consumption or backend execution. Hostile VM-start and container-restart
  regressions prove remote requests make no runner calls and consume no nonce.
- Progress (2026-07-24 capability-deadline hardening): armed cloud capabilities
  now treat `expires_at_ms` as an exclusive deadline, refusing a token at exact
  equality with the verifier clock rather than granting a boundary millisecond.
  The hostile BigBoy gate is green at 7/7; live libvirt/Podman evidence remains
  an external follow-up.
- Progress (2026-07-24 desired-state identity hardening): cloud reconciliation
  now accepts only regular JSON documents whose payload node/name match the
  canonical directory and filename, preventing a renamed or forged document
  from becoming an unretractable phantom workload. The hostile BigBoy reconcile
  suite is green at 13/13; live libvirt/Podman evidence remains external.
- Progress (2026-07-24 container staging filesystem hardening): Quadlet staging
  and rollback now reject symlinked directories/leaves and use atomic,
  non-following temporary writes, preventing a hostile stage tree from writing
  through to an outside victim. The focused `.90` container suite is green at
  20/20; live Podman/systemd evidence remains external.
- Progress (2026-07-24 desired-batch determinism hardening): `set-desired` now
  rejects duplicate declarations, duplicate removals, and declare/remove name
  collisions before the first filesystem mutation, eliminating ambiguous
  last-operation semantics and partial accepted batches. The focused BigBoy
  desired-state suite is green at 13/13; live libvirt/Podman evidence remains
  external.
- Progress (2026-07-24 container backend-error bounding): direct restart, logs,
  and destroy handlers now UTF-8-safely bound unavailable-backend error text
  before it enters cloud replies, including the selected instance context. The
  focused `.90` container lifecycle gate is green at 7/7; live libvirt/Podman
  evidence remains external.
- Progress (2026-07-24 cloud diagnostic boundary): the shared cloud runner now
  truncates backend summary lines at a UTF-8 boundary, preventing localized or
  hostile diagnostics from panicking the error path. The focused `.90` runner
  gate is green at 4/4; live libvirt/Podman evidence remains external.
- Progress (2026-07-24 cloud missing-body boundary): a Bus action with no body
  now receives a bounded error reply before legacy read compatibility can turn
  it into `{}`, preventing roster disclosure while preserving literal `{}` reads.
  The focused `.90` cloud gate is green at 139/139; live libvirt/Podman evidence
  remains external.
- Progress (2026-07-24 container option-boundary hardening): rootless container
  restart, logs, and destroy now reject leading-dash instance names before
  capability replay consumption or backend execution, preventing a workload
  target from becoming a `systemctl`/`journalctl` option. The focused `.90`
  lifecycle gate is green at 9/9; live Podman/systemd evidence remains
  external.
- Progress (2026-07-24 desired-reconcile fail-closed boundary): the per-node
  reconciliation planner now rejects malformed, foreign, or non-regular desired
  JSON documents before rendering tfvars or invoking the backend, while the
  best-effort mirror reader remains unchanged. The focused BigBoy reconcile
  suite is green at 14/14; live libvirt/Podman evidence remains external.
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

## Functional Completeness

### WL-FUNC-011 - Communications collaboration suite full replacement

- Status: Remaining
- Progress (2026-07-24 import-map version hardening): the legacy collaboration
  importer now rejects unsupported persisted ImportMap schema versions before
  replay or save, preventing a future map from silently changing idempotency
  semantics. The focused `.50` importer gate is green at 14/14; live media/LLM
  evidence remains external.
- Progress (2026-07-24 projection envelope hardening): the public collaboration
  projection boundary now rejects serialized signed envelopes above 256 KiB
  before opening its SQLite transaction, preserving atomic replay behavior for
  hostile inline payloads. The focused BigBoy projection gate is green at 10/10;
  live media/LLM evidence remains external.
- Progress (2026-07-24 replay ownership hardening): replayed `SpaceDeleted`
  events now require a currently present owner in the folded membership state,
  preventing a signed non-owner event from deleting a space read model. The
  focused projection gate is green at 9/9 and the full collaboration core gate
  at 50/50; live media/LLM evidence remains external.
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
  with a sealed key — needs an operator-provisioned key via mde-seal). These are
  non-blocking external evidence follow-ups. Criterion 12 (live
  visual signoff) is NO LONGER a block: the cutover shell is already deployed live on .15
  (boots drm:true, 12.1.0, stable, NRestarts=0), so its functional half is closed and the
  aesthetic signoff is now an optional operator glance, not a gate. 4 PRE-EXISTING
  (non-cutover) shell-test reds documented in docs/NEEDS-OPERATOR.md. Phase-3c follow-ups
  (editor CRDT/three-way-merge/review; call media plane) remain co-edit/hardware-gated.
- Progress (2026-07-24 projection identity hardening): signed presence events
  now require the payload actor to match the envelope actor before entering the
  global presence board, preventing a validly signed malformed event from
  impersonating another member. The focused projection regression is 1/1 and
  the full `mde-collab-core` library suite is 34/34.
- Progress (2026-07-24 call replay identity hardening): participant lifecycle
  events now require the payload actor to match the envelope actor during replay,
  preventing a signed malformed event from mutating another participant's call
  state. The focused farm projection gate is green at 5/5; the change is scoped
  to `mde-collab-core` and does not alter the live media-plane claim.
- Progress (2026-07-24 transfer authorization hardening): file-transfer control
  commands now require an active space and current membership before minting
  signed events. The focused regression and full collaboration-core farm suite
  pass at 1/1 and 38/38.
- Progress (2026-07-24 Files bridge authorization hardening): file-to-chat
  offers now use the daemon's authoritative node actor and reject malformed,
  cross-space, whitespace, or path-like peer targets before signing or writing
  an action. The focused bridge farm suite is green at 6/6; live multi-node
  chat/Bus round-trip evidence remains external.
- Progress (2026-07-24 live projection fold): the shell Communications mount now
  folds retained thread timelines/root lookups, alert inbox, file references,
  transfer jobs, clipboard lanes, and document sessions into the pure UI
  `CollabData` seam, with fail-soft clearing when the Bus is unavailable. The
  focused shell fold suite is green at 4/4. Core thread reads now scope roots,
  replies, and thread IDs to their requested space; the focused projection suite
  is green at 36/36, including deleted-root and cross-space leakage regressions.
- Progress (2026-07-24 convergent call state and seat-local read position):
  `SetCallMuted` now emits a signed `call_participant_muted` event for an active,
  connected, space-member participant; `SendDtmf` remains an explicitly
  ephemeral media-plane signal. Domain and SQLite folds retain the mute bit and
  replay it in arrival-order-independent read models. The focused type suite is
  green at 37/37 and the focused core suite is green at 36/36, including
  authorization, event round-trip, malformed-presence, and shuffled-replay
  regressions. The shell mount now derives each space's unread badge from the
  activity HLC after a durable seat-local cursor, persists that cursor under
  `local/collab/read-cursors` outside the replicated `state/collab/*` namespace,
  and advances it after the selected space renders. Its focused shell suite is
  green at 5/5, including reload persistence and post-cursor counting. This
  deliberately does not claim replicated read receipts.
- Progress (2026-07-24 call-space authorization hardening): call participant,
  mute, and end mutations now require the event `space_id` to match the active
  space before altering call state, preventing a valid member from mutating a
  different space's call. The focused BigBoy domain regression is green at 1/1.
- Progress (2026-07-24 projection ingress hardening): the public projection
  boundary now validates every event's schema version and signature before
  opening a transaction, so unsigned or future-schema replay batches cannot
  partially persist. Thread/root lookups are space-scoped, and replay rejects
  participant/presence payloads that impersonate another actor. The focused
  BigBoy `mde-collab-core` suite is green at 40/40; live media/SIP and sealed
  LLM evidence remain external follow-ups.
- Progress (2026-07-24 Communications stale-selection hardening): the call bar
  now revalidates its retained space selection against the current directory
  before offering `StartCall`, so a membership removal cannot leave an invalid
  actionable target. The focused Communications UI farm gate is green at
  52/52; live multi-node membership and media evidence remain external.
- Progress (2026-07-24 Communications stale-editor hardening): switching spaces
  now resets the embedded Markdown editor with the per-space selection, so a
  retained document view cannot continue rendering the previous space's
  content after membership loss or while the new projection arrives. The
  focused regression is green at 1/1 and the full `mde-collab-egui` suite is
  green at 52/52; live multi-node document evidence remains external.
- Progress (2026-07-24 call hang-up authorization hardening): `HangUpCall` now
  requires an active connected participant who is still a member of the target
  space, preventing a forged or stale command from ending another participant's
  call state. The focused pipeline gate is green at 2/2 and the full
  `mde-collab-core` suite is green at 41/41; live media evidence remains
  external.
- Progress (2026-07-24 legacy-import boundary hardening): the Communications
  migration reader now accepts only bounded regular files, rejects symlinked or
  non-regular sources, caps each source at 8 MiB, and refuses chat rings above
  500 entries. The focused BigBoy import gate is green at 13/13; live legacy
  filesystem migration evidence remains external.
- Progress (2026-07-24 call-answer membership hardening): `AnswerCall` and
  `DeclineCall` now require the caller to remain a member of the call's space
  before minting participant lifecycle events, closing a stale-call-id
  authorization path. The focused BigBoy pipeline regression is green at 3/3;
  `.170` farm rsync was ENOSPC, so the fallback gate is recorded explicitly.
- Progress (2026-07-24 call-mute replay identity hardening): replay now requires
  a `CallParticipantMuted` payload actor to match the signed envelope actor,
  preventing a validly signed malformed event from mutating another
  participant's mute state. The corrected full BigBoy `mde-collab-core` suite
  is green at 46/46, including the non-member answer regression and the new
  impersonation test; live media/SIP evidence remains external.
- Progress (2026-07-24 call-start replay identity hardening): replay now requires
  the `CallStarted` initiator to match the signed envelope actor, preventing a
  validly signed malformed event from making another actor appear to have
  started a call. The full BigBoy `mde-collab-core` suite is green at 47/47,
  including the new impersonation regression; live media/SIP evidence remains
  external.
- Progress (2026-07-24 departed-author mutation hardening): message edit and
  delete commands now require current membership in the target space before
  checking historical authorship, so a departed author cannot mutate an old
  message. The full `.50` `mde-collab-core` suite is green at 48/48; live
  media/SIP evidence remains external.
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
  bounded payloads, and paused/fix-loss behavior. The Maps shell now opens one
  Bus/Persist handle per refresh and folds all latest-wins feeds through it,
  preserving feed-local fail-soft behavior; the malformed-feed isolation and
  shared-handle suite is farm-green at 144/144, with the explicit `has_fix`
  no-fix regression independently green at 1/1. Car live proof also found and
  closed a `no-fix` telemetry spelling gap so nonzero coordinates cannot
  masquerade as a lock. AirNow evidence is green for
  mesh types 2/2, Maps/model 5/5, worker 7/7, worker-role census 19/19, and
  full Maps 136/136; its missing sealed key remains an honest unconfigured
  state with no network request or fabricated fetch time. Keyed feeds must idle
  honestly until operator-sealed free credentials exist. The follow-up overlay
  acceptance gate is green at 148/148: malformed retained AirNow snapshots and
  secret-store errors no longer paint stale markers, and idle traffic/wildfire
  layers render an explicit no-data state. The Map tab now presents all ten
  feed toggles in a grouped `Layers (N)` popover (Safety, Road & transit,
  Ambient), with its focused regression green at 1/1 (148 tests filtered).
  Provider attribution now wraps inside the map clip on narrow viewports rather
  than overflowing the left edge; the focused attribution proof is 1/1.
  FIRMS/AirNow live credentials and MG90-backed NCDOT/511 acceptance remain
  external evidence follow-ups, not blockers.
  The FIRMS lane is now a real credential-gated workstation worker rather than
  a schema-only placeholder: it resolves `secret:firms-api-key` through the
  sealed store, requires an explicit opt-in and fresh same-host vehicle fix,
  validates the strict NASA FIRMS HTTPS path, bounds the CSV response, filters
  finite in-radius rows, and publishes explicit unconfigured/secret-error/
  paused states. Its focused farm parser/contract suite is green at 8/8;
  The 2026-07-24 FIRMS parser hardening caps each CSV header/row at 64 fields,
  rejects empty or duplicate normalized headers before row parsing, and keeps
  over-wide hostile rows fail-soft by omitting them while retaining later valid
  hotspots. The official HTTPS probe response remains byte-bounded before CSV
  parsing; FIRMS has no UDP-equivalent payload path in this worker. These
  hostile-input regressions are included in the 8/8 farm suite.
  live hotspot acceptance remains correctly external until the operator seals
  a key and the MG90 reports a fresh fix. The Maps consumer now folds the
  retained FIRMS snapshot independently from NIFC and paints distinct
  orange/yellow hotspot markers with honest paused/stale/unconfigured/error
  badges; the shared Persist isolation proof is 1/1 and the focused FIRMS
  painter suite is 4/4. The stale NIFC badge no longer claims that a FIRMS key
  is required when that optional feed is simply unconfigured. A new
  read-only `install-helpers/verify-live-mirrors.py` proof tool validates the
  indexed Bus envelope, host identity, freshness, schema readiness, and
  envelope SHA-256 without contacting a gateway or feed; its deterministic
  self-test passes. It improves the external acceptance handoff but does not
  manufacture live feed credentials or MG90 fix evidence.
  The latest Maps reader pass now requires every retained overlay body to carry
  the selected node's exact `host` provenance, while FIRMS and NIFC remain
  independently folded; the full Maps suite is farm-green at 156/156 and the
  cross-node/FIRMS isolation regression is included. External feed credentials,
  fresh MG90 fix, and NCDOT acceptance remain optional live-data evidence
  follow-ups, not blockers to the autonomous implementation drain.
- Progress (2026-07-24 Caltrans retained-row hardening): the Maps consumer now
  drops malformed coordinates and non-finite/negative distances before paint and
  caps retained camera rows at 128, so hostile or oversized Bus snapshots cannot
  monopolize the map frame. The focused Caltrans farm gate is green at 5/5;
  live feed acceptance remains external.
- Progress (2026-07-24 Airspace consumer bound): the Maps Airspace reader now
  caps retained ready-mirror contacts at the typed 256-contact limit before
  conversion and painting, protecting the frame from an oversized persisted
  snapshot while preserving valid contacts. The focused `.90` consumer gate is
  green at 1/1 and the full Airspace module at 12/12; live scanner feed remains
  external.
- Progress (2026-07-24 live-mirror proof-tool no-follow hardening): the
  read-only verifier now rejects traversal, symlinked parents/leaves, and
  non-regular indexed files, then performs the final read with `O_NOFOLLOW` and
  `fstat` to close the check-then-read race. Its Python syntax and hostile
  symlink self-tests pass; it still contacts no gateway or external feed.
- Progress (2026-07-24 NCDOT contract hardening): malformed GeoJSON features are
  now isolated instead of poisoning valid incidents, point coordinate arrays
  are streaming-bounded to two or three values, and only `gps`/`dgps` fixes
  satisfy the fresh same-host context gate. The focused farm worker suite is
  green at 8/8; live MG90/NCDOT acceptance remains non-blocking evidence.
- Progress (2026-07-24 traffic provenance hardening): non-`Feature` GeoJSON
  members are now omitted with an explicit gap instead of being treated as
  incidents, preserving valid-event isolation. The focused BigBoy traffic
  worker gate is green at 9/9.
- Progress (2026-07-24 traffic stale-cache hardening): fetch/fix failures now
  publish an empty degraded projection instead of replaying incidents from a
  prior location; restart handling also scrubs an already-persisted stale
  snapshot while retaining the private last-good cache for a later 304. The
  focused BigBoy traffic worker gate is green at 9/9; live MG90/NCDOT evidence
  remains external.
- Progress (2026-07-24 forecast stale-cache hardening): NWS hourly forecast
  fetch/fix failures now publish an empty degraded projection instead of
  replaying samples from a prior location; restart handling also scrubs a
  persisted stale forecast while retaining the private last-good cache for a
  later 304. The focused BigBoy forecast worker gate is green at 9/9; live
  NWS/MG90 evidence remains external.
- Progress (2026-07-24 AirNow stale-cache hardening): refresh failures and
  missing fresh fixes now publish an empty degraded projection instead of
  replaying prior-location stations; restart handling also retracts a
  persisted stale AirNow record while retaining the private last-good cache.
  The focused `.50` AirNow worker gate is green at 9/9; live AirNow
  credentials/MG90 evidence remains external.
- Progress (2026-07-24 NWS-alert stale-cache hardening): failed refreshes,
  mismatched 304 responses, and missing fresh fixes now publish empty degraded
  projections instead of replaying prior-location alerts; restart handling
  also clears persisted stale alerts while retaining private conditional state.
  The focused `.90` NWS-alert worker gate is green at 13/13; live NWS/MG90
  evidence remains external.
- Progress (2026-07-24 ADS-B stale-cache hardening): failed refreshes,
  mismatched 304 responses, and missing fresh vehicle fixes now publish empty
  degraded projections instead of replaying prior-location aircraft; restart
  handling also clears persisted stale aircraft while retaining private
  conditional state. The focused BigBoy aircraft worker gate is green at
  11/11; live ADS-B/MG90 evidence remains external.
- Progress (2026-07-24 transit endpoint hardening): the production MBTA
  VehiclePositions feed is now restricted to the canonical HTTPS host, path,
  and no-query/no-credential URL shape; redirect coverage remains test-only so
  production cannot be pointed at an arbitrary endpoint. The focused transit
  farm suite is green at 11/11; live CDN fetch evidence remains external.
- Progress (2026-07-24 AirNow consumer-boundary hardening): the Maps AirNow
  painter now caps retained station iteration, rejects malformed coordinates,
  distance, AQI, and future/expired timestamps, refuses to paint retained
  stations under secret-store error states, and bounds provider labels. The
  focused BigBoy map suite is green at 10/10.
- Progress (2026-07-24 traffic consumer-boundary hardening): retained NCDOT
  snapshots now reject timestamps more than five seconds in the future, render
  an explicit invalid-timestamp state, and cap painting at the worker's 256
  event contract. The focused BigBoy traffic suite is green at 6/6.
- Progress (2026-07-24 Airspace repaint and mirror-boundary hardening): the
  visible Airspace panel now schedules a 33 ms repaint heartbeat independent of
  pointer movement, folds typed `state/airspace/<node>` snapshots as whole
  replacements, retracts stale selections, and refuses contacts from NoSource
  or Offline mirrors. The focused `.50` Airspace suite is green at 8/8; the
  alternate `.170` attempt was skipped after its farm filesystem reported
  ENOSPC. No MG90 scanner protocol or synthetic contacts were introduced.
- Progress (2026-07-24 home-destination display hardening): a validated US
  offline-gazetteer hit now always produces a visible Home destination label,
  falling back to the result title when a locality row has no subtitle. The
  focused `.90` Maps model proof is green at 1/1; no address is guessed when
  the opt-in local setting or gazetteer is absent.
- Progress (2026-07-24 destination-search viewport hardening): the Maps search
  surface now bounds its height by the remaining clipped viewport instead of
  forcing a 320 px minimum, keeping lower rows and controls inside short seats.
  The focused Maps view suite is green at 42/42; live seat visual proof remains
  external.
- Progress (2026-07-24 Advanced Maps viewport hardening): narrow Advanced pages
  now stack device cards at their actual clipped width, keep the destructive
  MG90 confirmation control inside the viewport, and retain bounded/revealed
  rail hit targets. The focused Maps view suite is green at 44/44, including
  the narrow-card and reset-action regressions; live visual proof remains
  external.
- Progress (2026-07-24 Advanced reveal-state hardening): leaving Advanced now
  clears its one-shot rail reveal marker even when the disclosure remains
  expanded, preventing a primary-tab selection from reusing stale scroll or
  hit-test state. The focused BigBoy Maps view gate is green at 49/49; live
  visual proof remains external.
- Progress (2026-07-24 Airspace consumer-boundary hardening): persisted Ready
  snapshots are revalidated at the Maps consumer boundary and malformed
  contacts are dropped before selection or painting, preserving the honest
  `NO SCANNER FEED` state without inventing an MG90 protocol. The focused `.50`
  Airspace gate is green at 9/9; live scanner configuration remains external.
- Progress (2026-07-24 transit stale-location hardening): when a refresh fails
  after the vehicle moves, the GTFS worker now discards the old nearby-vehicle
  snapshot and publishes an empty degraded snapshot for the new point instead
  of repainting stale vehicles at the wrong location. The focused `.90` transit
  suite is green at 12/12; live MBTA/MG90 evidence remains external.
- Progress (2026-07-24 Maps stale-selection hardening): choosing an out-of-range
  destination index now leaves search, preview, and the prior route selection
  untouched, preventing a stale UI index from reopening an old route. The
  focused BigBoy Maps model suite is green at 57/57; live visual proof remains
  external.
- Progress (2026-07-24 Airspace freshness hardening): daemon scans older than
  30 seconds now publish an empty Offline snapshot, while the Maps consumer
  retracts retained Ready contacts older than 15 seconds or materially
  future-dated. The daemon `.90` gate is green at 5/5 and the BigBoy Maps
  Airspace gate at 11/11; no undocumented MG90 scanner protocol was added and
  live scanner configuration remains external.
- Progress (2026-07-24 NWS alert timestamp hardening): the Maps alert consumer
  now treats a retained fetch timestamp more than five seconds ahead of the
  seat clock as stale, withholds its polygons and in-warning banner, and
  exposes no fabricated age. The focused BigBoy NWS alert gate is green at
  5/5; live NWS fetch credentials/configuration remain external.
- Progress (2026-07-24 overlay timestamp/paint-boundary hardening): retained
  aircraft and USGS earthquake snapshots now reject future timestamps, malformed
  coordinates, and unbounded event paint work; future data is withheld with an
  honest warning badge. The integrated BigBoy Maps library gate is green at
  190/190; live feed credentials/configuration remain external.
- Progress (2026-07-24 route-preview typography adoption): the Maps route
  preview title, destination summary, route cards, and Start control now use
  shared semantic typography roles instead of direct font literals, preserving
  existing geometry and hit targets. The focused `.50` route-preview gate is
  green at 4/4; live visual proof remains external.
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
- Progress (2026-07-24 Springboard input proof): a non-zero-origin headless
  pointer regression now exercises Back, Home, and Pin in both floating and
  docked modes and proves the foreground dock remains non-interactive outside
  its controls. The focused `.50` nav-bar gate is green at 1/1; live physical
  pointer/pixel evidence remains external.
- Progress (2026-07-24 status-bar boundary hardening): the status rail now clips
  paint/input to its reserved band and bounds the rollup cluster between the
  centered clock and right controls on narrow screens. A non-zero-origin,
  narrow-rail geometry/click regression is green at 16/16 on `.50`; live pixel
  comparison remains external.
- Progress (2026-07-24 Control Center viewport hardening): the Construct panel
  now clamps its card and inner content to short/narrow viewports, clips the
  scrollable paint/input region, and preserves scrim dismissal outside the
  visible card. The focused regression is green at 1/1 and the full Control
  Center test slice at 15/15; live physical pointer/pixel evidence remains
  external.
- Progress (2026-07-23): the home contract was reconciled to the operator's
  single-desktop directive: one untitled all-icons Desktop now flattens the
  canonical surface catalog, uses launcher-group accents for color coding, and
  has no title or page dots. Fleet & Mesh unifies Workbench, Mesh Map, and
  Explorer. The shell reserves top status-bar space for every workspace. A
  persistent black 240x56 floating navigation pill now provides Back, Home,
  Auto, and pin/dock actions; docked mode reserves a 56px left rail and uses a
  smooth slide-then-melt morph. Its headless render/accessibility proof passed
  6/6 on the farm.
  The cutover cleanup then replaced the stale RPM reachability check's deleted
  `dock.rs` input with the live `surfaces.rs` catalog; all eight embedded
  `mde-*-egui` crates pass the static shipped-and-reachable check, and the
  script self-test is green. Shared style/shell proof is farm-green at 245/245
  for `mde-egui`, 4/4 for the current shell surface-catalog tests, and
  1,715/1,715 for the preceding full `mde-shell-egui` run; the production Rust
  `taskbar` grep is empty.
  A follow-up headless layout proof now drives the real `central_view` and
  asserts that every workspace begins exactly below the 24px status bar; the
  focused farm test is 1/1. Live Car/MG90 acceptance, live switcher/physical
  acceptance, and physical VDI proof remain open as non-blocking evidence
  follow-ups. A read-only recheck of `.15` found the live shell still active
  with `NRestarts=0`, owning `/dev/dri/card1` and `/dev/tty1`, with
  HDMI-A-1 connected. The seat has `/usr/bin/ffmpeg`'s KMS grabber. Root KMS
  access can now open the XR30/10-bit framebuffer; the explicit
  `x2rgb10le -> hwdownload -> rgb24` path emits a 1920x1080 PNG, but the
  Intel X-tiled modifier is preserved as visible striping, so that file is not
  a valid visual proof. The VAAPI detile alternatives were also exercised and
  either reject the 10-bit RT format or preserve the same invalid pixels.
  Sunshine is configured for `capture = kms` but its user service is inactive
  and no live Moonlight client is connected, so this audit records no new
  screenshot or physical-pixel claim. The obsolete headless preview helper was
  updated to launch the live `mde-shell-egui` binary and its gallery now
  captures the current Construct home view; this repairs the proof tool but does
  not satisfy the physical-pixel gate. The read-only
  `verify-live-mirrors.py` helper also records exact Bus envelope identity,
  age, readiness, and provenance hashes for live Car/overlay handoff; its
  deterministic self-test passes. It is evidence tooling only and does not
  replace the unavailable valid DRM pixel capture.
  The PhonesHub mutation publisher now mints the same root-only capability as
  the KDC responder for pairing, phone actions, and enrollment-token issuance,
  while roster/browser reads remain open; its current focused shell farm suite
  passes 23/23.
  The live pixel proof was then repaired on `.15` with an opt-in linear GBM
  scanout allocation: the active XR30 framebuffer switched from Intel X-tiled
  to modifier `0`, and the resulting 1920x1080 RGB PNG was visually intact
  (SHA256 `48f43a115f55acf99fb3c85859cb86e30a1ced5454035280b7b49b92657e10f3`).
  The runtime override and test binary were removed; the packaged shell is
  active again with its normal service configuration and `NRestarts=0`.
- Progress (2026-07-22): interrupted U10/U11/U23 work was recovered into the main
  tree: the persistent Springboard base, slim top status bar, shared
  Workbench `NavigationBar`, and shared Console/Workloads style tokens. Farm
  evidence: status bar 8/8, Workbench 16/16, Console 47/47; the repaired
  integrated shell run passed **1,706/1,706** tests with zero failures. The three
  salvaged dirty Claude worktrees were removed after zero-unique-commit
  verification. Remaining acceptance is deterministic render/pixel capture and
  live VDI behavior; human visual review is informative, not a gate.
  On 2026-07-24 the refreshed Fedora 44 `magic-mesh-12.1.0-1` and
  `magic-mesh-browser-12.1.0-1` RPMs were deployed to non-production seat `.15`
  with complete declared dependency resolution. `rpm -V` is clean;
  `mackesd.service`, `mde-shell-egui.service`, `nebula.service`,
  `virtqemud.service`, and `virtnetworkd.service` are active, with zero
  restarts for the two primary services. Construct reports `12.1.0 ...
  2026-07-23`, and the shell owns
  `/dev/dri/card1` + `/dev/tty1` with HDMI-A-1 connected at 1920x1080. The
  deployed shell binary SHA256 is
  `5d7ff4467c3698923a3fa7e254c428ec918b60c97f0921d431498dc6753429d8` and
  the daemon SHA256 is
  `e08a46dc72735bfbd54ecf720ae6e2b2d1dd1137ab80c4865cda840ba46613e4`.
  Root-owned rollback copies of both pre-12.1.0 binaries remain in `/var/tmp`. A
  reversible non-production boot-policy override produced a live DRM
  Springboard frame (1920x1080 XR30, detiled PNG SHA256
  `ae0f05f63e0a7e1a6361994fe1e90543ca8833f802c85c6313844b855feef05b`):
  top status bar and Workbench/Mesh Map/Infra as Code tiles; the page dots
  visible in that capture belong to the pre-cleanup paged implementation and
  are not current acceptance evidence. No legacy bottom taskbar was present.
  The override was removed and the secure curtain
  restored (detiled PNG SHA256
  `b8971f07a000357c3edf645420df14462a697f024aabd5a9bec754ba981da34c`), with
  the service still active and `NRestarts=0`. Pixel capture and all-pages/VDI
  behavior remain open. The surface-card wire/auth regression suite is
  farm-green at 7/7.
- Progress (2026-07-24 live DRM pixel proof): the packaged shell was exercised on
  `.15` with a reversible root-only proof override: `require_login_at_boot` was
  temporarily disabled in the isolated Bus preference and
  `MDE_DRM_LINEAR_SCANOUT=1` was supplied through a temporary systemd drop-in.
  After the boot sequence settled, the native KMS grab produced a 1920x1080
  8-bit RGB PNG with SHA256
  `4773c06a2dd7d2391a92839bd78d2bff205f645d608200b8f82875a595a9f22a`.
  Pixel inspection shows the current post-cleanup untitled all-icons Desktop,
  Fleet & Mesh tile, separated 24px top status bar, and black floating navigation
  pill; no legacy taskbar or Desktop title is present. The temporary preference
  and drop-in were removed, the secure boot curtain is restored, and
  `mackesd`, `mde-shell-egui`, Nebula, and modular libvirt services are active
  with shell `NRestarts=0`. This closes the fresh Construct Desktop pixel proof;
  live VDI full-resolution behavior and live switcher/physical acceptance remain
  open. A fresh Car frame was also captured under the same reversible proof
  path (SHA256 `abccf68e14102674626ed4c95e5ef9b31d66be59aa5f381d4f2e14860170d175`);
  MG90 direct-control and driving-data acceptance remain open.
- Progress (2026-07-24 single-desktop cleanup): `springboard.rs` now projects
  `Surface::ALL` directly into one canonical desktop and no longer carries
  page index, offset, settle spring, page dots, horizontal page-swipe, or page
  navigation state. Horizontal drags are inert; keyboard selection remains
  lock-step across the complete icon catalog, and the upper pull-down keeps its
  Spotlight seam. The current production-shaped Springboard suite is farm-green
  at 20/20, including a real pointer click on the canonical Fleet & Mesh tile;
  full integration, switcher-snapshot, and VDI proof remain open.
- Progress (2026-07-24 VDI chrome seam): focused Desktop requests are now an
  explicit immersive layout state. The central workspace no longer reserves
  the docked 56px rail, and the navigation pill is not painted over a focused
  guest; the existing Escape/return chord remains the exit path. A focused
  `central_view` integration assertion is in the farm gate; live VDI
  full-resolution behavior for a live guest and the `.15` VDI pixel proof remain open.
- Progress (2026-07-24 navigation hit-test repair): the floating navigation
  `egui::Area` had a full-screen interactive rectangle while painting only the
  visible pill/rail, so its transparent foreground widget intercepted home and
  workspace clicks. `nav_bar.rs` now bounds the Area to the animated bar
  footprint, translates control rectangles into Area-local coordinates, and
  retains screen-space painting/accessibility geometry. The regression first
  caught egui's persisted default `600x400` Area size, then passed after the
  explicit size was fixed: the focused farm nav suite is now 8/8, including a
  real workspace-underlay click regression, and the Fedora 44 release build is
  green. The verified shell artifact
  (`712d33e3b922535c71a6403634ed636513e581f41f08a879805cf4c6a5a9c463`) is
  now deployed to `.15` with a root-owned rollback copy; the DRM service is
  active, owns `/dev/dri/card1` and `/dev/tty1`, and has `NRestarts=0`. Direct
  pointer click-through on the physical seat remains unclaimed; the earlier
  surfaceless `egui_glow` startup panic is retained as separate runtime
  evidence rather than being confused with this hit-test defect.
- Progress (2026-07-24 switcher snapshot slice): the production switcher now
  caches the real rendered body of each expanded surface as it is left, using
  the shared offscreen egui PNG backend and the shell's existing texture upload
  path. Cards use those captured textures when available and retain the honest
  accent plate only when capture is unavailable; Desktop continues to prefer a
  live VDI decoder frame. The focused BigBoy switcher suite is green at 16/16,
  including a regression proving that one real snapshot replaces only its own
  card plate. The package-wide BigBoy shell suite is now green at 1,728/1,728;
  live VDI full-resolution proof remains open.
- Progress (2026-07-24 switcher snapshot F44 cutover): the current source was
  packaged in the Fedora 44 builder with base/browser/lighthouse payload gates
  green, and the matching base/browser RPMs passed `.15`'s separate
  `rpm -Uvh --test` transaction before installation without `--nodeps`. After
  the expected transient post-install systemd transport failure, explicit
  service recovery brought `mackesd`, `mde-shell-egui`, Nebula, `virtqemud`,
  and `virtnetworkd` to `active`, with zero restarts for the two primary units;
  `rpm -V` and executable `ldd` checks are clean. The deployed current shell
  SHA256 is `889d12e594e3d012592cfc4d723e7375b4e092b8b7c276f27702b89ccb407662`
  and the daemon remains `e08a46dc72735bfbd54ecf720ae6e2b2d1dd1137ab80c4865cda840ba46613e4`.
  Real switcher snapshot code is now on `.15`; live VDI full-resolution proof
  remains open.
- Progress (2026-07-24 switcher crash repair): live `.15` journal evidence traced
  the nav-triggered `egui_glow` texture panic to the offscreen switcher capture
  creating a second wgpu GL/surfaceless context in the live DRM process. The
  shared capture backend now requests Vulkan only, preserving the live EGL/GL
  context and retaining the honest fallback plate when Vulkan is unavailable.
  The focused capture farm gate passes 2/2 (the no-adapter host takes the
  documented skip path), and the Fedora 44 base/browser RPM payload gates pass.
  The matching RPMs were transaction-tested and installed on `.15`; the
  deployed shell hash is `9e5bf98d51869bfef43ec1f32138c07bff7df0ce025f2055ca77bc65a103aa4c`,
  startup reports `drm=true`, `NRestarts=0`, and no repeat `surfaceless`, texture,
  or panic log has appeared. A physical browse/nav replay remains the final
  live interaction proof.
- Progress (2026-07-24 crash repair + physical switcher proof): `.15` briefly
  exited to getty/bash because the shell painted `FontFamily::Name("heading")`
  while that transient named family was unbound during an appearance handoff.
  The known-good shell was restored immediately, then the Car heading paints
  were changed to the always-bound proportional family and the corrected F44
  RPM was built on BigBoy (`926ec547...` base, `1e9a70b6...` browser; payload
  gates green). The corrected shell is deployed at `.15` with SHA256
  `d2c323662be2d3a702ac19248dd0ca70c2b2e73dc61ef4a5de39faff795f5888`;
  `rpm -V`, `ldd`, and post-deploy journal checks are clean, the shell is
  active with `NRestarts=0`, and no repeat panic is present. A fresh reversible
  linear-GBM DRM run then emitted `before.png`
  (`48628a0f...`) and `after.png` (`68e16fd7...`); a physical uinput
  split-frame Super-release/Tab sequence produced the actual switcher overlay
  with a live `Car Home` card instead of Spotlight. The proof power override
  and systemd drop-in were removed and the persisted profile restored to
  Construct. Live VDI full-resolution proof remains open.
- Progress (2026-07-24 farm gates): the post-cleanup Springboard suite passes
  19/19 on BigBoy, and the current-source workspace/VDI layout suite passes
  9/9 on `.90`, including the immersive full-screen assertion. Farm formatting,
  worklist lint, and documentation-supersession lint are green. The later F44
  cutover entry records the completed `.15` service deployment; fresh live
  pixel and physical VDI proof remain open.
- Progress (2026-07-24 ABI gate): the native Fedora 42 farm RPM was deliberately
  rejected by `.15`'s dry-run because its FFmpeg/FLAC/graphics sonames are older
  than the live Fedora 44 seat. The documented Fedora 44 container RPM cut was
  dispatched on farm `.90`; no `--nodeps` install was attempted. The live seat
  remains on its known-good package until the matching artifact passes
  `rpm -Uvh --test`.
- Progress (2026-07-24 F44 deployment): the matching Fedora 44 base and browser
  RPMs passed `.15`'s strict dry-run and were installed without `--nodeps`.
  Package hashes are base `0bde12022c8c106cd0aac245ae9458492e7b4733a77b1daae12c58aed96cff8c`
  and browser `7b1a96fdbe1631d2af8abfa3e5238d518349167e306e9cfe5ace77a26e353c83`.
  `rpm -V` is clean; explicit post-install recovery restarted `mackesd` and
  `mde-shell-egui` after their normal graceful stop, with both primary services
  at `NRestarts=0` and Nebula/libvirt services active. Deployed shell and daemon
  hashes are `d27e19339b01a2d77ec8efc68d25b5978cf39f18e8e9f349a7936fb87a8badb2`
  and `e08a46dc72735bfbd54ecf720ae6e2b2d1dd1137ab80c4865cda840ba46613e4`.
  No reboot was performed; fresh VDI proof remained open at that stage and is
  tracked by the later live-DRM evidence below.
- Font depth follow-up (2026-07-24): add the five typography improvements to
  the shared Construct/Car design-token work instead of tuning individual
  screens in isolation:
  1. establish a stronger type hierarchy with explicit display, headline,
     title, body, label, and caption sizes plus deliberate weight changes;
  2. use a more expressive font treatment by selecting the embedded face's
     appropriate weights/optical sizes and reserving the mono face for data,
     code, and telemetry rather than using one flat weight everywhere;
  3. tune tracking, kerning, and line-height per token and per size so labels
     do not look cramped while headings retain a clean, intentional shape;
  4. improve text/surface contrast and add restrained text depth only where it
     supports hierarchy on elevated or translucent materials, with no glow or
     shadow that harms legibility;
  5. make rasterization and responsive scaling part of the font contract,
     covering DPI/font-scale changes, fallback glyphs, and the `.15` DRM
     capture path at the supported display sizes.
  Acceptance is shared-token implementation in `mde-egui`, no ad-hoc
  per-surface font literals for canonical chrome, WCAG AA contrast checks for
  interactive text, headless layout/render regressions at 100/125/150/200%,
  and a fresh `.15` pixel review confirming crisp glyphs without clipping or
  fallback-family surprises.
- Progress (2026-07-24 shared typography implementation): `mde-egui` now
  exposes a shared `TypographyRole` contract with semantic family/size tokens,
  optical tracking, explicit line heights, and painter-compatible layout jobs.
  Construct Desktop tile labels, switcher headers, and the top status bar now
  use that contract instead of flat painter font literals. The focused shared
  farm suite is green at 246/246, and the full shell binary gate is green at
  1,735/1,735 with zero failures.
- Progress (2026-07-24 navigation typography adoption): the shared NavigationBar,
  Toolbar, and Sidebar now resolve all canonical chrome labels through semantic
  typography roles, with no direct font literals remaining in `nav_chrome.rs`.
  The focused shared navigation suite is green at 5/5; geometry and interaction
  behavior are unchanged.
- Progress (2026-07-24 Construct/Car typography adoption): Car Home and Control
  Center now resolve their canonical display, headline, title, body, label, and
  caption paints through the shared `TypographyRole` contract. Existing anchor
  coordinates and hit-target geometry are preserved, while sparse Car values
  remain honest rather than rendering blank live-looking labels. Direct canonical
  `FontId` literals are absent from both surfaces. Focused farm gates are green:
  Control Center 16/16 and Car Home 7/7, with no failures.
- Progress (2026-07-24 live container probe): `.15` had only about 324 MiB free
  on its 15 GiB root filesystem before the proof. A bounded Podman
  `busybox:1.36.1` pull failed honestly with `no space left on device`, I/O
  errors under `/var/lib/containers/storage`, and a Podman `storage.lock`
  panic; SSH then began resetting during key exchange and the remote Sunshine
  ports were no longer listening. No broad cleanup or reboot was attempted;
  recovery now needs the physical console or a separately available management
  path before the live container proof can resume.
- Progress (2026-07-24 container safety hardening): the Workloads container
  path now refuses deployment below a 512 MiB free-space floor before staging
  or pulling, rolls back newly staged Quadlets after a failed operation, and no
  longer passes an ineffective `container` role tag. The focused farm suite is
  green at 18/18; live `.15` success remains correctly unclaimed until recovery.
- Progress (2026-07-24 switcher viewport hardening): the complete 15-card recent
  ring now compresses card previews to remain inside short viewports, and swipe
  thresholds use each card's actual height. The focused `.90` switcher suite is
  green at 17/17; live physical-pointer and full-resolution VDI proof remain
  external evidence.
- Progress (2026-07-24 switcher invalid-texture boundary): zero-sized retained
  snapshot handles now fall back to the honest accent plate instead of painting
  a blank preview surface after a failed or empty capture. The focused BigBoy
  switcher gate is green at 21/21; live physical-pointer and full-resolution
  VDI proof remain external evidence.
- Progress (2026-07-24 status-rail geometry hardening): the Volume, Network, and
  Brightness hit targets retain their normal macOS-sized geometry on workstation
  rails while compressing safely inside narrow headless/windowed rails. The
  focused `.50` status-bar gate is green at 13/13; no live seat change was made.
- Progress (2026-07-24 release-identity hardening): the root Cargo workspace
  version is now the single platform release authority; the shared theme derives
  About, watermark, and splash release text from `CARGO_PKG_VERSION`, while the
  Bash welcome, status snapshot, and `mesh-help` reflect installed RPM metadata.
  The isolated browser workspace manifests are synchronized to the same release
  value and the dependency-free welcome self-test plus shell syntax checks are
  green. RPM installation/reflection and live seat visual comparison remain
  external release evidence; no live seat change was made.
- Progress (2026-07-24 chooser typography adoption): the protocol badge now
  resolves its caption font through the shared `TypographyRole` contract,
  removing the remaining canonical ad-hoc font literal in that chooser surface
  while preserving badge geometry. The focused `.50` chooser gate is green at
  98/98; no live seat change was made.
- Progress (2026-07-24 shell typography adoption): Console entry rows now use
  shared Body/Caption roles, and Storage tooltips, action labels, and fallback
  glyphs now use the shared Caption role instead of direct font sizes. Focused
  farm gates are green at Console 48/48 and Storage 34/34; no live seat change
  was made.
- Progress (2026-07-24 System typography adoption): Settings choice tiles and
  mixer status rows now resolve their body/caption fonts through the shared
  `TypographyRole` contract, preserving existing geometry and interaction. The
  focused BigBoy System gate is green at 74/74; no live seat change was made.
- Progress (2026-07-24 System Bluetooth typography adoption): Bluetooth adapter
  and device rows, scan controls, metadata, trust controls, and status copy now
  resolve through shared caption/body roles instead of ad-hoc font literals.
  The focused BigBoy shell System gate is green at 74/74; no live seat change
  was made.
- Progress (2026-07-24 splash typography adoption): the boot splash product,
  studio, and release labels now use shared Display/Headline/Caption roles,
  keeping release identity and geometry unchanged. The focused `.50` splash
  gate is green at 6/6; no live seat change was made.
- Progress (2026-07-24 console typography adoption): all 15 remaining direct
  canonical font literals in the Console surface now resolve through shared
  `TypographyRole` helpers, preserving geometry and hit targets. The focused
  BigBoy Console gate is green at 48/48; no live seat change was made.
- Priority: P1
- Complexity: Epic
- Problem: The workstation chrome is Win10-shaped (48px bottom taskbar + tray
  flyouts, `src/dock/mod.rs`) with an ephemeral search launcher and no home
  screen, after three chrome reversals in ten days; no single design standard
  governs the shell, and the operator's locked direction (ADR-0006: Apple HIG
  as principles, iPadOS structure + macOS pointer manners) has no
  implementation.
- Required outcome: Construct per `docs/design/platform-interfaces.md` Part I -
  persistent untitled all-icons Desktop (one canonical grid with
  `LAUNCHER_GROUPS` color accents, no title, no page dots, no dock, no widgets),
  slim top status bar, Control Center, Notification Center,
  Spotlight (Front Door engine, keyboard flow byte-identical), card app
  switcher with snapshot previews, shared
  NavigationBar/Toolbar/Sidebar/Sheet/Popover components adopted by all
  canonical surfaces, scrim materials + HIG radii + zoom-from-tile motion, two-profile
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
  the untitled all-icons Desktop, status bar, Control Center, Notification
  Center, Spotlight, switcher with real snapshots, zoom/navigation-bar
  transitions, VDI full-res with auto-hidden bar; post-cutover grep gate (zero
  taskbar identifiers in production code). Human visual review is informative
  only.
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
- Progress (2026-07-24 Car keymap persistence hardening): persisted Car bindings
  now stream-discard unknown/non-bindable keys and malformed actions, preserving
  valid bindings and the legacy `go_phone` alias without allowing an unbounded
  active map. The focused shell Car keymap gate is green at 7/7; live MG90 and
  physical Car evidence remains external.
- Progress (2026-07-24 moving-Car glance budget): the Communications calls roster
  now limits the visible list to six entries while Car mode is in motion, with
  an explicit count of additional calls available when stopped; stationary and
  non-Car views retain the full roster. The focused Car slice is green at 4/4
  and the full Communications crate at 53/53 on `.50`; live MG90 and physical
  Car evidence remains external.
- Progress (2026-07-24 Car strip typography hardening): the six app-strip tiles
  now clip their painted content and shorten only visual labels at narrow widths
  while retaining full WidgetInfo route names and 44pt touch targets. The focused
  BigBoy narrow-strip gate is green at 1/1; live MG90 and physical Car evidence
  remains external.
- Progress (2026-07-23 live mirror proof): the MG90 gateway at `172.20.0.25`
  is reachable from the network (ping and TCP/2222), and `.15`'s active
  `mackesd` is publishing a fresh `state/vehicle/Basement-Test-Workstation`
  mirror from that gateway. A direct read-only Bus sample is `online=true`,
  MGOS `4.3.0.1`, battery `13.0V`, cellular `-96dBm`, with an honest
  `fix_type=no-fix`, zero satellites, and zero speed; no stale or fabricated
  motion claim is being promoted. `.15` has both `mackesd` and the DRM shell
  active (`NRestarts=0` for the shell). The direct MG90 SSH service accepts
  connections on port 2222 but the available key is not authorized, so the
  remaining live proof is a read-only `.15` DRM capture with the fresh mirror;
  no credential was guessed.
  The newest read-only Bus payload sampled from `.15` remains fresh and online:
  MGOS `4.3.0.1`, battery `13.0V`, cellular `-93dBm`, `fix_type=no-fix`, zero
  satellites, and zero speed. The live mirror remains present and honest; the
  prior KMS capture limitation was repaired by the later linear scanout proof
  below, while direct `.25` SSH credentials and real driving/fix data remain
  external gates.
- Progress (2026-07-23): the production map model now exposes a
  `refresh_from_persist` seam and a deterministic live/stale vehicle Bus fixture
  covering MG90 speed, battery, GNSS, WAN, and dashboard fold (including a six
  second-old stale row); the focused fixture passed 1/1 and the full Maps suite
  passed 145/145 on BigBoy. Live `.25` SSH remains credential-blocked; the
  existing non-production `.15` DRM capture remains the available visual
  evidence. Retained MG90 telemetry now expires after five seconds;
  stale/simulated readings re-dash, cannot drive motion policy, and keep their
  provenance/age honest. Vehicle Fuel/Odometer now share the same live gate.
  Farm `mde-maps-location-egui` suite passed 97/97. Settings now consumes
  `glance_clamp` to shorten the moving-car rail without hiding destinations from
  search/menus, while host-down power prompts consume `deferred_notice` and emit
  no action until stopped (Lock stays available). The focused Car policy suite
  passed 8/8 and the moving Settings paint/defer regression passed 1/1 on farm
  `.130`. At that stage remaining was live MG90 + DRM-seat proof; human review
  is informative only. A non-production `.15` DRM capture (fresh shell PID 2307515) proved the
  Car dashboard, left-third 10-tile instrument strip, and Nav/Media/Music/
  Comms/Vehicle/Settings strip with an online MG90 mirror; detiled PNG SHA256 is
  `54b9f22469c53b2d514e7baf8ba5a2ce0c1574cc29167efcda95bcd762bf9b56`. The same
  capture exposed a `fix_type:no-fix` spelling gap; Maps now rejects that form
  with a regression test, so those nonzero coordinates cannot paint as a GPS
  lock. The seat was restored to workstation layout with the temporary boot
  override removed, service active, and DRM ownership confirmed.
- Progress (2026-07-24 live Car pixel proof): with the `.15` vehicle mirror
  publishing fresh `online=true`, MGOS `4.3.0.1`, `fix_type=no-fix`, zero
  satellites, and zero speed, a reversible Car-profile + linear-GBM proof run
  produced a 1920x1080 RGB PNG (SHA256
  `abccf68e14102674626ed4c95e5ef9b31d66be59aa5f381d4f2e14860170d175`). Pixel
  inspection shows Auto Mode, the full left instrument strip with honest no-fix
  dashes, Navigation/Media/Telematics glance cards, and the six-app strip. The
  appearance profile, secure boot curtain, and temporary systemd drop-in were
  restored afterward; Construct is active again with the five required services
  active and shell `NRestarts=0`. This closes the fresh `.15` Car pixel handoff;
  direct MG90 SSH credentials and real driving/fix data remain external gates.
- Progress (2026-07-24 MG90 access update): the operator confirmed full MG90
  SSH access. The gateway is now also validated through its authenticated LCI
  at `172.20.0.25`: MGOS `4.3.0.1`, main battery `13.00V`, cellular WAN up,
  ignition `on`, eight satellites in view but zero usable, and GPS antenna
  `Disconnected`. The live read-only LCI session proves the device and
  management plane are reachable; the current root-SSH credential path still
  rejects the configured ESN password, so the worker records an honest SSH GPS
  gap rather than fabricating a fix. Real driving/fix acceptance remains open
  until the antenna has a lock and the authorized SSH path is exercised.
- Progress (2026-07-24 MG90 communication-plane adapter):
  `install-helpers/mg90-access.sh` is now the canonical bounded entry point for
  pinned root SSH (`ssh-probe`/`ssh-exec`), authenticated MG-LCI (`lci-get`), the
  separate `:11532` application server (`app-get`), and read-only TCP inventory.
  The current host proves `ssh-up`, `lci-up`, and `app-up`; a strict root probe
  reaches the MG90 host but returns `Permission denied`, so no credential bypass
  is claimed. The adapter requires root-only password files, never places a
  password in argv, pins the verified `[172.20.0.25]:2222` ED25519 key, and
  documents the GPS/OBD/HDOBD/GPIO/Acetech application surfaces. The Rust worker
  now prefers the same root-only SSH password-file contract and feeds the
  password over stdin. Farm `mackesd` vehicle tests passed 20/20 (including the
  LCI ignition parser), and the LCI/application endpoint parsers remain the next
  cutover before OBD or application telemetry can leave the honest-gap state.
- Progress (2026-07-24 MG90 guide reconciliation): the Sierra Wireless AirLink
  MG90 Software Configuration Guide Rev 6 confirms a higher-value documented
  path we were not using: Status Broadcast emits a selectable UDP JSON beacon
  carrying location, GPIO, WAN, GNSS fix/satellites/antenna, VPN, ignition,
  battery, and temperature; GPS configuration also supports NMEA/TAIP local TCP,
  UDP, serial, and remote forwarding with threshold-driven intervals. The
  adapter now exposes receive/connect commands for those streams and records the
  guide as the protocol authority. The live unit's broadcast/GPS forwarding
  configuration is not yet read-only verified, so this remains an optional
  evidence follow-up, not a cutover blocker or claimed live feed.
- Progress (2026-07-24 status-beacon implementation): `mackesd` now accepts the
  documented MG90 Status Broadcast UDP JSON stream through the bounded
  `MDE_VEHICLE_STATUS_PORT` listener, validates coordinates/telemetry ranges,
  prefers beacon battery/temperature/ignition/GNSS fields, and preserves LCI,
  NMEA, and explicit stream-gap provenance when the beacon is malformed or
  absent. The farm vehicle suite passes 22/22, including override and malformed
  payload regressions. Live broadcast configuration and antenna/fix acceptance
  remain open as non-blocking evidence follow-ups; no synthetic fix is promoted.
- Progress (2026-07-24 bounded-status hardening): Status Broadcast datagrams now
  have a 16 KiB receive bound with truncation and invalid-UTF-8 rejection, while
  out-of-range satellite, voltage, and temperature fields are dropped without
  silent clamping and LCI/NMEA fallbacks remain intact. The focused farm vehicle
  suite is green at 24/24; live broadcast configuration and GNSS lock remain
  non-blocking evidence follow-ups.
- Progress (2026-07-24 wildfire consumer-boundary hardening): the retained NIFC
  overlay now rejects future-dated snapshots and malformed coordinates, bounds
  perimeter/polygon/ring/point work before tessellation, and fails soft to an
  honest no-data or stale badge. The focused `.50` wildfire suite is green at
  7/7.
- Progress (2026-07-24 MG90 parser boundary hardening): GPGGA now verifies a
  supplied NMEA checksum, rejects invalid coordinate/quality/satellite/HDOP/
  altitude ranges, and preserves coordinate-free no-fix samples without
  inventing a position. Empty Status Broadcast payloads now fail as a schema
  gap and retain authenticated LCI values. The focused worker suite is green at
  25/25 and the mesh-type vehicle suite at 8/8; no undocumented `:11532`
  application response schema was guessed or promoted into telemetry.
- Progress (2026-07-24 MG90 adapter request boundary): the canonical adapter now
  validates local absolute paths, rejects scheme-relative URLs, traversal,
  fragments, whitespace, and control characters, bounds login redirects, and
  never follows redirects for the authenticated status fetch. The dependency-
  free adapter syntax and self-test pass; the live `:11532` application schema
  remains credential-gated and is not promoted into telemetry.
- Progress (2026-07-24 Car sparse-data and touch-boundary hardening): empty or
  whitespace telemetry now falls back to honest descriptors, undersized viewports
  fail closed before producing off-body targets, and all six Car strip routes
  retain 44pt touch targets. The focused Car farm suite is green at 7/7; live
  MG90 telemetry and rendered instrument-strip proof remain external evidence.
- Progress (2026-07-24 Car Auto-mode degraded-state hardening): an undersized
  workspace now renders an actionable “Resize workspace to use Auto Mode” notice
  instead of silently showing only the title. The focused BigBoy Car binary gate
  is green at 8/8.
- Progress (2026-07-24 Car accessibility-boundary hardening): dashboard cards
  and six app-strip targets now expose labeled button semantics, visible focus
  rings, and shared Enter/Space activation in addition to pointer taps. The
  focused BigBoy Car Home gate is green at 8/8; live visual evidence remains
  external.
- Progress (2026-07-24 Car navigation route hardening): the large Car Navigation
  tile now selects the Maps `Drive` tab while the Vehicle tile remains on the
  `Vehicle`/OBD telematics tab, so Navigation cannot open the telematics view or
  strand the driver without a route-home path. The focused shell route test is
  green, with the Car Home suite at 7/7; live MG90/seat evidence remains
  external.
- Progress (2026-07-24 firmware-fixture honesty): the simulated firmware workflow
  no longer reports package integrity as a passing placeholder; it now renders an
  explicit unverified warning and has a focused regression preventing false
  verification wording. Live MG90 firmware checks remain hardware-gated and no
  live evidence is claimed.
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
