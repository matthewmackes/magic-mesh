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

## Current Snapshot - 2026-07-28 release gate

- **11 active epics:** 11 `Remaining`, 0 `Blocked`; no `Needs clarification`.
- **Release gate (last integrated wave):** the 2026-07-28 integrated wave passes the full `mackesd` farm
  suite at **4,124 passed, 0 failed, 1 ignored**. The Fedora 44
  base/browser/thin-lighthouse RPM cut is green at **81.7 / 39.1 / 11.0 MiB**;
  the exact base RPM requires the F44 FFmpeg ABI (`libavcodec.so.62`,
  `libswresample.so.6`, `libswscale.so.9`).
- **Live public mesh:** `mcnf-clean-20260728` has three thin public
  DigitalOcean lighthouses at `104.236.118.177`, `46.101.219.245`, and
  `64.23.131.57`. Lighthouses are `magic-mesh-lighthouse` only, use the small
  profile, run `12.1.1`, and form a healthy three-member etcd quorum at
  `10.42.0.1` through `10.42.0.3`.
- **Seats:** Dell (`172.20.146.225`, overlay `10.42.0.4`), seat 15
  (`172.20.0.15`, overlay `10.42.0.5`), and Eagle (`172.20.146.145`, overlay
  `10.42.0.6`) are enrolled in the new mesh. All three run `magic-mesh-12.1.1`
  with active `nebula` and `mackesd`; a founder-side overlay ping matrix
  reaches all six overlay members. Seat 15's lighthouse mappings were
  refreshed to the new public endpoints after enrollment.
- **P0:** WL-ARCH-007 (authorization mint + direct lifecycle proof),
  WL-FUNC-011 (optional real media/LLM evidence remains), WL-FUNC-016
  (native mesh clipboard lanes for seat/browser/VDI), WL-UX-010
  (Mesh Teams near-parity interface redesign), and WL-UX-011 (unified local
  hardware and connectivity controls).
- **In flight:** WL-FUNC-012 live map feeds, WL-UX-007 Car, WL-UX-009 unified
  workspace design language, and WL-UX-010 Mesh Teams
  near-parity UX. WL-UX-011 owns the newly reviewed This Node hardware-center
  consolidation and missing workstation/laptop controls. The 2026-07-23
  thin-lighthouse policy is enforced in role pinning, onboarding, install
  profiles, directory discovery, DNS, workers, secret scope minting, and both
  media helpers; no new lighthouse may carry media or file-sharing duties.
- **Non-blocking external evidence:** WL-FUNC-011 still has optional real
  second-peer/SIP and sealed DigitalOcean model demonstrations. These proofs no
  longer block autonomous implementation or the active drain; missing resources
  remain explicitly recorded below.
- **Archived by this takeover:** WL-DOC-004, WL-FUNC-013, and WL-RUN-008 in
  `docs/worklist-archive/2026-07-22-platform-takeover.md`; WL-SEC-005 and
  WL-BUILD-004 are archived in `docs/worklist-archive/2026-07-23-thin-drain.md`;
  WL-SEC-007 is archived in `docs/worklist-archive/2026-07-24-sec007-closure.md`;
  WL-SEC-006 is archived in `docs/worklist-archive/2026-07-26-sec006-closure.md`.

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

## Build, Installation, And Deployment

## Core Architecture

> WL-ARCH-001 (Remove OpenStack; OpenTofu+Ansible IaC) and WL-ARCH-006 (Workloads
> cockpit) both closed **DONE 2026-07-22** and moved to
> `docs/worklist-archive/2026-07-22-live-block-removal.md` (operator directive:
> remove the live-seat / OpenStack-removal live-apply blocks; both code-complete +
> IaC-validated).

### WL-ARCH-007 - Repair Workloads cockpit E2E wire, placement, and authorization

- Status: Remaining
- Progress (2026-07-26 lifecycle action/roster correlation): the read-only
  Workloads live-proof helper now correlates required VM roster evidence to the
  retained authorized lifecycle action when both proof seams are required. If
  the operator does not pass an explicit `--expect-vm`, a fresh valid
  `action/vm/lifecycle` target is inferred and the `event/vm/instances` roster
  must contain that same VM before the roster can count as lifecycle proof. This
  prevents an unrelated fresh roster from being accepted beside a valid action.
  The helper still redacts HMAC token material and remains read-only. Evidence:
  local `python3 -m py_compile install-helpers/verify-workloads-live-proof.py &&
  install-helpers/verify-workloads-live-proof.py --self-test` passed; farm `.50`
  slot `workloads-proof-correlation` ran the same bytecode/self-test proof and
  passed; scoped `git diff --check` passed for the helper.
- Progress (2026-07-26 Workloads Plan verb contract): the Workloads UI
  `plan_provision` action now publishes the dedicated versioned `plan` Bus
  request contract for the selected placement node instead of reusing the live
  `provision` verb. The focused shell test
  `iac::tests::provision_plan_emits_dedicated_plan_request_contract` proves the
  request lands on `action/cloud/plan`, carries `schema_version=1` and
  `node=eagle`, carries no `armed_token`, and emits zero `action/cloud/provision`
  requests. Evidence: BigBoy farm lane
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=workloads-plan
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui
  provision_plan_emits_dedicated_plan_request_contract -- --nocapture` passed
  1/1 test (1827 filtered).
- Progress (2026-07-26 onboard open-broker proof correlation): the read-only
  Workloads live-proof helper now treats `event/onboard/apply` open-broker
  acknowledgements as proof only when the event is fresh, target/issuer/nonce
  shaped, carries a valid `open-broker <session>` applied entry, and correlates
  to a retained `action/onboard/apply` request with matching issuer, target,
  nonce, and `OpenBroker.session_id`. The proof output reports both event and
  action ULIDs/hashes while still redacting signed material; malformed, stale,
  orphaned, and action-after-ack cases fail closed. Validation passed local
  `python3 -m py_compile`, local
  `install-helpers/verify-workloads-live-proof.py --self-test`, local
  `git diff --check`, and synced-checkout farm runs on `.50` and `.90` of
  `python3 -m py_compile install-helpers/verify-workloads-live-proof.py &&
  install-helpers/verify-workloads-live-proof.py --self-test`. No Bus messages
  were published and no live host state changed.
- Progress (2026-07-26 explicit peer placement live-proof): the Workloads
  live-proof helper now expands an explicit `--node <name>` into both `<name>`
  and `peer:<name>` candidates, matching the deployed Bus convention on `.15`
  without broadening beyond the same placement identity. The helper self-test
  now proves an explicit `node-a` argument accepts a fresh
  `peer:node-a` `event/vm/instances` roster while still redacting lifecycle
  tokens. Validation passed local `python3 -m py_compile`, local
  `install-helpers/verify-workloads-live-proof.py --self-test`, and the focused
  `.50` synced-checkout helper gate. A patched read-only live run on
  `Basement-Test-Workstation` (`172.20.0.15`) proves `mackesd` active at
  `NRestarts=0`, root-only cloud-arm credential/drop-ins OK,
  `state/cloud/Basement-Test-Workstation` fresh at **14.091 s** with
  `construct_cloud`, `opentofu=up`, `ansible=up`, `libvirt=up`, and apply
  armed, Podman `5.8.4` active, bootstrap SSH reachable, and a fresh
  `event/vm/instances` roster for `peer:Basement-Test-Workstation`
  **5.330 s** old with zero instances. The run still exits non-zero when
  lifecycle/onboard proofs are required because `.15` has no retained
  `action/vm/lifecycle` message and no successful `event/onboard/apply`
  open-broker acknowledgement for either explicit candidate. KVM remains
  honestly unclaimed: `/dev/kvm` is absent and CPU `vmx/svm` is absent, while
  the libvirt `default` network and `mde-vms` pool are active.
- Progress (2026-07-26 lifecycle op allowlist verifier): the read-only
  Workloads live-proof helper now refuses retained `action/vm/lifecycle`
  evidence whose `op` is outside the real VM lifecycle contract
  (`create`, `start`, `stop`, `pause`, `resume`, `destroy`, `attach_usb`,
  `detach_usb`, or read-only `refresh`). The self-test proves an unknown op is
  blocked while HMAC token material remains redacted. Validation is green via
  local `python3 -m py_compile`, helper `--self-test`, and the `.50`
  `proof-helpers` synced-checkout farm gate. No live lifecycle action or
  destructive Workloads mutation was taken.
- Progress (2026-07-26 lifecycle-action verifier freshness): the read-only
  Workloads live-proof helper now treats retained `action/vm/lifecycle`
  evidence as fresh only when its Bus index timestamp is an integer within the
  configurable `--max-lifecycle-action-age-seconds` window, and rejects far
  future timestamps beyond a 30 s skew allowance. The helper still redacts
  retained HMAC tokens in both text/JSON evidence. Validation passed local
  `python3 -m py_compile install-helpers/verify-workloads-live-proof.py`,
  local `install-helpers/verify-workloads-live-proof.py --self-test`, and the
  focused `.50` farm slot 0 verifier gate after sync. No live `.15` state,
  credentials, or service restarts were touched.
- Progress (2026-07-25 post-hotfix `.15` Workloads verifier recheck): after
  installing the 2026-07-25 `mackesd` hotfix binary on
  `Basement-Test-Workstation` and force-clearing the stale stop-sigterm service
  cgroup, the required live Workloads proof still exits 0 over direct root SSH.
  `mackesd` and `mde-shell-egui` are active with `NRestarts=0`; the encrypted
  cloud-arm credential remains root-only; `state/cloud/Basement-Test-Workstation`
  is fresh at **5.959 s** with `opentofu=up`, `ansible=up`, `libvirt=up`, and
  apply armed; Podman `5.8.4` is active. KVM remains explicitly not claimed:
  `/dev/kvm` is absent and the CPU exposes no `vmx/svm` virtualization flag.
- Progress (2026-07-25 integrated `.15` Workloads verifier recheck): after the
  integrated `mackesd` release binary
  `61c0c202f39cb12286e1c69e2b2d74c0c947eecd7124c73dbcf4acd782b4bbaf` was
  installed on `.15`, the old daemon again hung during restart and was recovered
  with the bounded forced systemd path. The required read-only verifier still
  exits 0 over direct root SSH: `mackesd` and `mde-shell-egui` are active with
  `NRestarts=0`; the encrypted cloud-arm credential and systemd drop-ins remain
  OK; `state/cloud/Basement-Test-Workstation` is fresh at **44.108 s** with
  `construct_cloud`, `opentofu=up`, `ansible=up`, `libvirt=up`, and apply
  armed; Podman `5.8.4` and `podman.socket` are active. KVM remains a warning,
  not a claim, because `/dev/kvm` is absent and the CPU still exposes no
  `vmx/svm` virtualization flag.
- Progress (2026-07-25 live `.15` Workloads backend remediation/proof): live
  `Basement-Test-Workstation` (`172.20.0.15`) was read-only probed with
  `install-helpers/verify-workloads-live-proof.py`, then remediated in-place
  where the missing pieces were normal package/service state. Installed
  `opentofu` and `libvirt-daemon-kvm`, reclaimed explicit disposable disk
  pressure only (`dnf`/PackageKit/libdnf caches, bounded journal/log growth,
  stale `/root/.local/share/mde/bus.pre-chat-bus-root-20260717-212313`), and
  restarted `mackesd` once to re-arm tripped breakers and refresh backend
  health. Rootfs improved from **243 MiB free / 99% used** to **2.4 GiB free /
  84% used**. The required proof now exits 0 with `mackesd` and
  `mde-shell-egui` active at **NRestarts=0**, root-only encrypted
  cloud-arm credential and systemd drop-ins OK, fresh
  `state/cloud/Basement-Test-Workstation` mirror **14.189 s** old with
  `opentofu=up`, `ansible=up`, `libvirt=up`, apply armed, Podman
  `5.8.4` active, local SSH active, and libvirt `default` network plus
  `mde-vms` pool active. The remaining KVM evidence is honestly not claimed:
  this seat still reports `/dev/kvm` absent and no CPU `vmx/svm` virtualization
  flag, so direct KVM lifecycle remains a firmware/hardware exposure follow-up.
- Progress (2026-07-25 Workloads live-proof verifier harness):
  `install-helpers/verify-workloads-live-proof.py` now provides a bounded,
  read-only WL-ARCH-007 evidence collector for placement hosts. It opens the
  Bus SQLite index in read-only mode, follows the indexed message envelope with
  no-follow bounded reads, redacts retained `action/vm/lifecycle` HMAC tokens,
  and checks the cloud-arm credential presence/permissions, fresh
  `state/cloud/<node>` health, `event/vm/instances`, `event/onboard/apply`
  open-broker acknowledgements, Podman socket/version, libvirt default network
  and `mde-vms` pool, `/dev/kvm`, CPU virtualization flags, and bootstrap SSH
  reachability without publishing actions or starting/stopping services. Local
  verifier validation passed `python3 -m py_compile
  install-helpers/verify-workloads-live-proof.py`,
  `install-helpers/verify-workloads-live-proof.py --self-test`, and a
  no-require read-only dry run. The dry run was only harness validation on the
  dev host: it found no `/run/mde-bus` or cloud-arm credential, reported Podman
  socket active, found libvirt reachable with `default` network active and
  `/dev/kvm` present, but `mde-vms` absent. Live `.15` Workloads/libvirt
  lifecycle/bootstrap closure remains external until the verifier is run on an
  installed placement host with the required evidence flags.
- Progress (2026-07-25 first-desktop lean feature boundary): the
  `mackesd --no-default-features` library path no longer pulls the async daemon
  surface into first-desktop tests. Bus/local-applier effects stay behind
  `async-services`, worker restart-policy metadata has a lean registry tag,
  Nebula enrollment uses the real filesystem peer-directory fallback without
  `substrate`, mesh-init fails honestly when Nebula materialization is requested
  from a lean build, and heartbeat etcd writes remain async-only. The BigBoy
  no-default first-desktop gate is green at **17/17**; the async-services
  first-desktop gate is green at **21/21** after pre-initializing the temp Bus
  store used by the Bus lifecycle roster test; touched-file rustfmt is green
  and local diff-check is clean. Live root credential, libvirt backend,
  bootstrap SSH, Podman/Cuttlefish, and installed seat evidence remain
  external.
- Progress (2026-07-25 first-desktop authorized lifecycle publisher): live
  first-desktop placement now publishes signed `vm_lifecycle` Create/Start
  actions on `action/vm/lifecycle`, binds each request to the cloud-arm HMAC
  token digest, waits for `event/vm/instances` to report the VM running, and
  only then opens the broker session through the existing remote-push path.
  Tests cover injected placement ordering, worker parser/token-gate acceptance,
  Bus publish/readback, and running-roster observation; the final BigBoy
  `mackesd --lib onboard::first_desktop --features async-services` gate is green
  at **21/21**, and touched-file rustfmt is green. The follow-up lean feature
  boundary above closes the earlier `--no-default-features` probe failure. Live
  root credential, libvirt backend, bootstrap SSH, Podman/Cuttlefish, and
  installed seat evidence remain external.
- Progress (2026-07-25 first-desktop lifecycle bridge): the first-desktop
  planner now carries an explicit VM name, placement peer, golden image path,
  libvirt sizing, and network, and folds the placement into the existing
  `vm_lifecycle` `Create` then `Start` wire actions instead of a parallel cloud
  model. The BigBoy `onboard::first_desktop --features async-services` gate is
  green at **18/18**. Live authorized Bus publish, roster observation, bootstrap
  SSH, and libvirt/Podman/Cuttlefish evidence remain external.
- Progress (2026-07-25 signed onboarding transport): `BusApply` now signs and
  publishes a bounded JobBundle, waits for a matching target acknowledgement,
  and rejects unrelated or expired responses. `OpenBroker` now carries full
  session context, and the target `LocalApplier` persists the exact
  `SessionRequest::Open`; focused remote-push tests pass **24/24**, the target
  onboard worker passes **12/12**, and the integrated BigBoy `mackesd` gate is
  **4,124 passed, 0 failed, 1 ignored**. Bootstrap SSH plus live
  libvirt/Podman/Cuttlefish evidence remain external.
- Progress (2026-07-25 container-lifecycle argv boundary): container lifecycle
  helpers now terminate option parsing before systemd unit targets and bind
  journal targets as `--unit=...`, preserving valid names while preventing
  option-shaped targets from becoming commands. Focused lifecycle tests are
  green at **9/9**; the integrated BigBoy mackesd gate is **4,117 passed, 0
  failed, 1 ignored**. Live libvirt/Podman lifecycle evidence remains external.
- Progress (2026-07-25 lifecycle-argv boundary): Workloads lifecycle verbs
  now reject option-shaped instance targets beginning with `-` before
  authorization/replay consumption or virsh execution. Focused cloud
  regression is green at **1/1**; integrated BigBoy mackesd gate is green at
  **4,117 passed, 0 failed, 1 ignored**. Live libvirt/Podman lifecycle evidence
  remains external.
- Progress (2026-07-25 desired-state write/preflight boundary): Workloads
  desired writes now reject serialized documents above 256 KiB before creating
  any state tree, and `set-desired` preflights every existing canonical JSON
  document with bounded no-follow regular-file reads before the first batch
  mutation. The focused desired-verb farm gate is green at **16/16**, the
  focused reconcile gate at **18/18**, and the integrated BigBoy `mackesd`
  gate is green at **4,115 passed, 0 failed, 1 ignored**. Live
  libvirt/Podman lifecycle evidence remains external.
- Progress (2026-07-25 image-catalog boundary): Workloads image roster and
  promotion now reject symlinked image roots/version directories, bounded
  manifest/sidecar reads use no-follow regular files, artifact discovery skips
  links and special files, and promotion replaces marker/sidecar leaves
  atomically. The focused image farm gate is green at **16/16** and the
  integrated BigBoy `mackesd` gate is **4,113 passed, 0 failed, 1 ignored**;
  live libvirt/KVM lifecycle evidence remains external.
- Progress (2026-07-25 desired-document no-follow boundary): cloud
  reconciliation now opens desired documents with no-follow, non-blocking,
  close-on-exec descriptors before the existing 256 KiB bounded read, so a
  raced final symlink cannot become plan input. The focused reconcile farm
  gate is green at **42/42**, and the integrated BigBoy `mackesd` gate is
  **4,107 passed, 0 failed, 1 ignored**; live libvirt/KVM lifecycle evidence
  remains external.
- Progress (2026-07-25 malformed plan boundary): Workloads `plan` now rejects
  malformed or oversized JSON before backend access; the intentional `{}`
  worker-local plan request remains valid. The focused desired-verb farm gate is
  green at **15/15** and the integrated BigBoy `mackesd` gate is **4,103 passed,
  0 failed, 1 ignored**. Live libvirt/KVM lifecycle evidence remains external.
- Progress (2026-07-25 lifecycle apply contract): authorized lifecycle verbs
  now fail closed when a backend reports `ok=true` without `applied=true`, and
  destructive desired-state records are retained instead of being retracted
  after an unapplied delete. The focused cloud suite is green at **164/164**;
  the merged BigBoy `mackesd` gate is **4,101 passed, 0 failed, 1 ignored**.
  Live libvirt/KVM lifecycle evidence remains external.
- Progress (2026-07-25 live-provision desired-state bridge): authorized
  Workloads `provision` now renders the selected node's strict persisted
  desired slice into fresh tfvars before `tofu apply`, preventing stale or
  missing Android/Cuttlefish state from reaching OpenTofu. The focused cloud
  gate is green at **163/163**; live libvirt/Podman/Cuttlefish evidence remains
  external.
- Progress (2026-07-25 Android VM preparation contract): the Workloads
  cockpit now exposes the existing typed `android-provision` Bus contract with
  explicit node placement, default naming, typed confirmation, and an honest
  `desired saved` result that does not claim a live Cuttlefish VM. The focused
  `.90` shell gate is green at **50/50**; live libvirt/Podman/Cuttlefish
  evidence remains external.
- Progress (2026-07-25 VDI session persistence boundary): replicated VDI
  session records and roaming monitor layouts now use bounded no-follow
  regular-file reads with UTF-8, special-file, symlink, and growth rejection
  before JSON materialization. The integrated session-broker farm gate is green
  at **30/30** and the focused roaming-layout gate at **51/51**. Live
  libvirt/Podman evidence remains external.
- Progress (2026-07-25 subprocess capture boundary): timeout-bounded command
  execution now drains stdout/stderr concurrently, retains at most 64 KiB per
  stream, drains excess, and still kills/reaps timed-out children. The
  integrated farm gate is green at **8/8**. Live libvirt/Podman evidence
  remains external.
- Progress (2026-07-25 console endpoint authority boundary): Workloads console
  hosts are now bounded, delimiter-safe, malformed-input rejecting, and IPv6
  addresses are bracketed before URI materialization. The focused cloud-console
  farm gate is green at **12/12**. Live libvirt/Podman evidence remains
  external.
- Progress (2026-07-25 VM key and IP-mirror boundary): compute provisioning now
  reads generated Nebula key material and local `.nebula-ip` mirrors through
  bounded descriptor-backed regular-file readers that reject final symlinks,
  non-regular leaves, oversized content, and invalid UTF-8. The focused BigBoy
  compute-provision gate is green at **39/39**; the integrated BigBoy
  `mackesd --lib --features async-services` gate is green at **4,045 passed,
  0 failed, 1 ignored**. Live libvirt/Podman evidence remains external.
- Progress (2026-07-25 cloud-arm credential boundary): the root Workloads
  shell plus daemon ActionAuthorizer, cloud gate, KDC-host, and remediation CLI
  now load the systemd cloud-arm credential through descriptor-backed,
  regular-file readers that reject final symlinks and inputs above 4 KiB before
  decoding while preserving the root-process gate. The Workloads/IAC farm suite
  is green at **47/47**; the integrated BigBoy `mackesd --lib
  --features async-services` run exercised **4,017 passed, 1 failed, 1
  ignored**, with the unrelated storage shutdown timing test passing in an
  isolated rerun (**1/1**). Live libvirt/Podman evidence remains external.
- Progress (2026-07-24 bounded cloud I/O): Workloads now drains stdout/stderr
  concurrently to EOF with a 1 MiB cap and UTF-8-safe truncation, rejects
  oversized inventory/output JSON before parsing, and bounds prior Quadlet
  rollback reads with `O_NOFOLLOW` plus a 1 MiB cap. BigBoy cloud tests are
  green at **181/181**; live libvirt/Podman evidence remains external.
- Progress (2026-07-25 desired-document read boundary): cloud reconciliation
  now reads desired documents through a 256 KiB bounded regular-file path before
  JSON materialization. Best-effort mirror reads skip an oversized sibling while
  strict reconciliation fails closed; the integrated BigBoy `mackesd --lib`
  gate is green at **3,998 passed, 1 ignored**. Live libvirt/Podman evidence
  remains external.
- Progress (2026-07-25 fleet-store read boundary): revision YAML, apply-ack
  JSON, and one-shot nudge payloads now use bounded descriptor-backed regular
  file reads that reject final symlinks, non-regular leaves, oversized input,
  and invalid UTF-8 before parser materialization. The full farm
  `magic-fleet --lib` gate is green at **56/56**; live libvirt/Podman evidence
  remains external.
- Progress (2026-07-25 jobs/validation read boundaries): replicated job
  templates, run manifests, target results, validation manifests, and
  reachability rows now use bounded descriptor-backed regular-file reads that
  reject final symlinks, non-regular leaves, oversized input, and invalid UTF-8
  before YAML/JSON materialization. Focused farm evidence is green at **7/7**
  jobs tests and **6/6** validation tests; the combined `magic-fleet --lib`
  gate is green at **61/61**. Live libvirt/Podman evidence remains external.
- Progress (2026-07-25 lifecycle persistence boundary): replicated service
  lifecycle requests and results now use bounded descriptor-backed regular-file
  reads that reject final symlinks, non-regular leaves, oversized input, and
  invalid UTF-8 before JSON materialization. The focused lifecycle farm
  invocation is green at **74/74**, and the integrated BigBoy `mackesd --lib
  --features async-services` gate is green at **4,054 passed, 0 failed, 1
  ignored**. Live libvirt/Podman evidence remains external.
- Progress (2026-07-25 fleet-log read boundary): replicated JSONL fleet logs now
  use bounded descriptor-backed regular-file reads that reject final symlinks,
  non-regular leaves, oversized input, and invalid UTF-8 before line parsing.
  The focused farm gate is green at **9/9**, and the integrated
  `magic-fleet --lib` gate is green at **65/65**. Live libvirt/Podman evidence
  remains external.
- Progress (2026-07-25 runner/CLI read boundaries): runner event snapshots and
  CLI baseline/exception inputs now use bounded descriptor-backed regular-file
  reads that reject final symlinks, non-regular leaves, oversized or growing
  input, and invalid UTF-8 before parsing. The focused runner gate is green at
  **2/2**, the CLI gate at **3/3**, and the integrated `magic-fleet
  --all-targets` farm gate is green at **67 library, 3 binary, and 3
  multiprocess tests**. Live libvirt/Podman evidence remains external.
- Progress (2026-07-25 service-directory read boundary): replicated KDC service
  rows now use bounded descriptor-backed regular-file reads that reject final
  symlinks, non-regular leaves, oversized input, and invalid UTF-8 before JSON
  materialization while preserving fail-soft sorting. The focused farm gate is
  green at **8/8**, and the integrated KDC-host library gate is green at
  **96/96**. Live libvirt/Podman evidence remains external.
- Progress (2026-07-24 image provider boundary): `image-build` now caps raw
  requests before verb parsing, bounds replicated promotion/SHA sidecars,
  limits roster rows, and keeps reply/error/raw-log text bounded. The focused
  oversized-request regression is green at 1/1; the full BigBoy `mackesd`
  library gate is green at 3,992 passed, 1 ignored.
- Progress (2026-07-24 inventory read-boundary hardening): cloud inventory and
  output reads now cap host/output rows, group counts, and display text before
  `CloudReply` materialization while preserving sensitive-value masking. The
  focused BigBoy inventory gate is green at 60/60; live libvirt/Podman evidence
  remains external.
- Progress (2026-07-24 inventory parser boundary hardening): inventory JSON is
  now rejected above 1 MiB before `serde_json` materialization, with bounded
  diagnostics preserved for the caller. The focused inventory gate is green at
  14/14 and the full BigBoy `mackesd` library gate at 3,997 passed, 1 ignored;
  live libvirt/Podman evidence remains external.
- Progress (2026-07-24 direct-libvirt action framing hardening): the lifecycle
  Bus parser now rejects action bodies above the shared 64 KiB RPC limit before
  JSON materialization, with the oversized-body regression covered by the
  focused BigBoy `vm_lifecycle` gate at 55/55. Direct libvirt/KVM live evidence
  remains external.
- Progress (2026-07-24 cloud action boundary hardening): the shared cloud action
  parser now rejects bodies above the RPC limit before JSON materialization, and
  container deploy rejects oversized image, port, environment, and volume
  fields before authorization or Quadlet staging. The full BigBoy `mackesd`
  library gate is green at 3,973 passed, 1 ignored; direct libvirt/KVM live
  lifecycle evidence remains external.
- Progress (2026-07-24 live rootless Quadlet lifecycle proof): the `.15`
  placement seat ran a real Podman 5.8.4 rootless Quadlet generated from a
  temporary `.container` unit: systemd user start, restart, journal retrieval,
  and the same `systemctl --user disable --now` cleanup operation used by the
  worker all completed successfully. The temporary unit, container, and pulled
  Alpine image were removed afterward; no live success is claimed for the
  unavailable libvirt/KVM backend. This closes the
  remaining live Podman/systemd evidence follow-up while the direct libvirt
  lifecycle remains an external firmware/backend proof.
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
- Progress (2026-07-25 desired-body framing hardening): `set-desired` and its
  capability-target parser now reject caller bodies above the shared 64 KiB RPC
  limit before JSON materialization or filesystem mutation, while the valid
  legacy `{}` plan/read shape remains accepted. The focused BigBoy desired gate
  is green at 42/42; live Podman/libvirt evidence remains external.
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
- Progress (2026-07-26 Mesh Teams near-parity survey lock): the operator
  re-scoped Communications toward near Microsoft Teams parity for mesh
  operators under the user-facing `Mesh Teams` name. Locked decisions: a
  Teams-like visual model, one large integrated push, Teams + Channels as the
  organizing hierarchy, direct/group messages rendered as channels, a Teams-style
  app rail with the operator set (Activity, Teams, Calls, Files, Alerts,
  Transfers, Clipboard, Settings), global Activity as the attention inbox,
  Alerts folded into Activity while remaining rail-reachable, Transfers and
  Clipboard available as contextual panels while remaining rail-reachable,
  channel tabs for Posts/Files/Calls, rich details pane, multi-line rich
  composer, `Ctrl+Enter` send with Enter newline, composer file/clipboard/
  document create-and-attach, local-only reactions, local channel find but no
  global Mesh Teams search, resolve/reopen threads, side thread pane, shared
  pins plus private saved messages, ad-hoc channel meetings with channel Posts
  as the meeting discussion, no recording/transcription, WebRTC P2P media first,
  disabled-but-visible device controls until real providers enumerate, one
  share flow with remote-control escalation, small-group scale (2-20), transfer-
  first files, full IDE as the default document experience, real-time document
  coauthoring required for completion, basic channel tasks/action items, full
  two-way external Discord server integration, and glance-safe Car Mode limited
  to alerts and calls. Explicit exclusions: @mentions, message priority,
  scheduled messages, global search, emoji/GIF/sticker expression, slash/workflow
  commands, recordings, and transcripts. This lock supersedes older WL-FUNC-011
  details where they conflict, including flat spaces-only navigation, eight
  primary mode tabs, Enter-to-send, one-pane Documents default, and suite-wide
  search language. Linked visible-interface owner: WL-UX-010; this epic remains
  the backing contract/worker/read-model owner.
- Progress (2026-07-26 Discord bridge status read-model seam): the shared
  Communications read side now has a typed `DiscordBridgeBoard` with bounded
  bridge rows carrying overall configuration state, Discord-to-Mesh and
  Mesh-to-Discord directional flow status, provenance, and degraded detail. The
  model distinguishes unconfigured, provider-unavailable/degraded, and
  configured rows without calling Discord or fabricating external servers.
  Evidence: `.50` slot `discord-types-rerun` focused `cargo test -p
  mde-collab-types discord_bridge -- --nocapture` passed **1/1**; `.90` slot
  `discord-ui-rerun` focused `cargo test -p mde-collab-egui discord_bridge --
  --nocapture` passed **2/2**; `.170` slot `discord-fmt-rerun` touched-file
  `rustfmt --edition 2021 --check --config skip_children=true` passed.
- Progress (2026-07-26 seat .15 Activity-tab performance): Communications /
  Mesh Teams no longer publishes or paints an unbounded Activity history on
  open. The core Activity read model now publishes only the newest 1,024
  per-space rows while preserving newest-last order, and the egui Activity body
  filters once then renders through `ScrollArea::show_rows` so a large retained
  feed lays out only visible rows. Regression coverage includes a 2,000-row
  Activity fixture that must keep painted shapes bounded. Farm evidence is green:
  `.90` `cargo test -p mde-collab-egui activity -- --nocapture` **4/4**,
  `.170` `cargo test -p mde-collab-core activity_feed -- --nocapture` **1/1**,
  `.170` broader `cargo test -p mde-collab-core projection -- --nocapture`
  **17/17** after updating the stale transfer fixture to mark transfers active
  before pausing, and `.170` scoped touched-file `rustfmt --edition 2021
  --check`.
- Progress (2026-07-26 transfer-control state boundary): collaboration-core
  transfer admission now folds each transfer's current ledger state into the
  domain aggregate and rejects stale or malicious `ControlTransfer` commands
  outside that state: queued transfers can only cancel, active transfers can
  pause/cancel, paused transfers can resume/cancel, and terminal completed/
  failed/canceled transfers carry no controls. Farm gates are green on `.50` at
  **1/1** focused `cargo test -p mde-collab-core
  transfer_controls_follow_the_current_ledger_state -- --nocapture` and on
  `.170` for touched-file `rustfmt --edition 2024 --check`. Full package
  `cargo fmt -p mde-collab-core -- --check` was not claimed because it still
  reports unrelated pre-existing `projection.rs` formatting drift outside this
  slice.
- Progress (2026-07-25 worker merge/log backpressure boundary): the
  collaboration worker now keeps its own merge slices below the core
  all-or-nothing 4,096-envelope cap by batching retained Bus and actor-log input
  at 1,024 events. Durable JSONL actor-log replay now streams through a
  `BufReader`, skips oversized lines before serde/projection, avoids directory
  symlink traversal by checking entry file types, and does not materialize an
  unbounded retained log into memory before merge. Farm gates are green:
  `.130` `cargo test -p mackesd --lib --features async-services
  workers::collab::tests -- --nocapture` at **24/24**, `.90` focused
  `merge_batch_chunks_oversized_retained_input` at **1/1**, and `.170`
  touched-file rustfmt. Live WebRTC/SIP/LiveKit/media and sealed DO/LLM
  provider demonstrations remain external evidence follow-ups, not blockers.
- Progress (2026-07-25 call media provider-proof seam): the retained
  `state/collab/call-media-verification` publisher now consumes an explicit
  in-daemon `CallMediaProviderRegistry` instead of a hardcoded null transport.
  The registry is bounded to one verifier per adapter family and defaults empty
  in `CollabWorker`, preserving honest `transport_unavailable` /
  `provider_unavailable` rows until a WebRTC/SIP/LiveKit provider is registered.
  Registered providers must return observed advancing frame/data deltas before a
  row can become `live_media_verified`; waiting-for-peer rows still short-circuit
  before provider access. Farm gates are green on `.130` BigBoy at **2/2**
  focused worker `call_media` tests plus **4/4** `collab_media` verifier tests,
  and `.90` touched-file `rustfmt --edition 2024 --config skip_children=true
  --check`. Live WebRTC/SIP/LiveKit/media proof remains external.
- Progress (2026-07-25 call media-verifier seam): Communications now publishes a
  retained `state/collab/call-media-verification` sidecar by consuming the
  global `state/collab/call-media-readiness` board. The verifier is bounded and
  emits one row per candidate adapter; single-seat readiness stays
  `waiting_for_connected_peer`, and adapter-ready rows fail honestly as
  `transport_unavailable` for missing WebRTC verifier transport or
  `provider_unavailable` for missing LiveKit/SIP gateway/provider until a real
  SIP/WebRTC/LiveKit transport is registered and proves advancing frames. Farm
  gates are green on `.90` at **37/37** `mde-collab-types --lib`, `.130`
  BigBoy at **3/3** `mackesd collab_media` plus **1/1**
  `call_media_readiness_is_published_for_the_local_media_adapter`, and `.50`
  touched-file rustfmt. Live WebRTC/SIP/LiveKit/media proof remains external.
- Progress (2026-07-25 media-readiness publish boundary): the mackesd
  collaboration worker now publishes the core `CallMediaReadiness` projection
  for the local actor as retained adapter-facing Bus state at
  `state/collab/call-media-readiness` and
  `state/collab/call-media-readiness/<space>`. Readiness rows now carry
  `CallMediaAdmission`, distinguishing signed-state `adapter_ready` from the
  honest degraded `waiting_for_connected_peer` single-seat state so future
  WebRTC/SIP/LiveKit workers can avoid treating one-seat tests as live remote
  media. Farm gates are green on `.90` at **3/3** `mde-collab-core
  media_readiness`, `.170` at **37/37** `mde-collab-types --lib`, `.130`
  BigBoy at **1/1** `mackesd --lib --features async-services
  call_media_readiness_is_published_for_the_local_media_adapter`, and `.50`
  touched-file `rustfmt --edition 2024 --config skip_children=true --check`.
  Live WebRTC/SIP/LiveKit/media proof remains external.
- Progress (2026-07-25 call media-readiness boundary): Communications now has a
  typed adapter-facing `CallMediaReadiness` read model that exposes only
  non-ended calls where the local actor is a connected participant, with bounded
  session and participant fan-out plus explicit candidate adapter/requirement
  rows. This prepares SIP/WebRTC/LiveKit admission without claiming live media.
  Farm gates are green at **2/2** `mde-collab-core media_readiness`, **1/1**
  `mde-collab-types read_model_variants_round_trip`, and touched-file rustfmt.
  Live WebRTC/SIP/LiveKit/media proof remains external.
- Progress (2026-07-25 DTMF media-admission boundary): ephemeral DTMF commands
  now require a connected call participant and validate the RFC 4733 keypad
  alphabet (`0-9`, `*`, `#`, `A-D`/lowercase) before any future SIP/WebRTC
  adapter can see the tone. The focused farm regression passed, touched-file
  rustfmt is green, and the BigBoy `mde-collab-core` suite is green at
  **72/72** plus doc-tests. Live WebRTC/SIP/LiveKit/media proof remains
  external.
- Progress (2026-07-25 DigitalOcean AI sidecar boundary): Communications AI
  suggestions now carry a bounded caller `request_id`, have a typed
  `cancel_ai_suggestion` command, validate targeted context as belonging to the
  request space before provider admission, and publish bounded
  `state/collab/ai-requests` rows with DigitalOcean-only provider attribution.
  Without cloud consent the worker publishes an honest failed sidecar row and
  emits no signed collaboration history or provider call; consented test requests
  can be canceled by request id. Farm gates are green at **37/37**
  `mde-collab-types`, **1/1** focused `mde-collab-core` admission, and **2/2**
  BigBoy `mackesd --lib --features async-services ai_request`; live WebRTC/SIP,
  sealed DigitalOcean key/provider wiring, and LiveKit/media proof remain
  external.
- Progress (2026-07-25 no-peer call boundary): Communications now rejects stale
  selected spaces and disables call-start controls when the selected space has
  no other member, showing an honest no-peer state instead of emitting a call
  target. The full Communications suite passes **77/77**; live
  WebRTC/SIP/LiveKit/media and real partition evidence remain external.
- Progress (2026-07-25 collaboration input boundary): Communications send,
  edit, and thread-reply editors now cap restored and pasted UTF-8 input at the
  256 KiB command-body limit before layout or action emission, preserving valid
  boundaries and showing a review notice when clipping occurs. The focused UI
  gate is green at **3/3**, and the integrated `mde-collab-egui` gate is green
  at **76 passed, 0 failed, 0 ignored**. Live WebRTC/SIP and sealed DO/LLM
  evidence remain external.
- Progress (2026-07-25 collaboration message-body boundary): the command
  pipeline now rejects inline message, edit, and thread-reply bodies above
  the existing 256 KiB projection contract before signing, ID allocation, or
  HLC consumption; the exact boundary remains accepted. Focused regression is
  green at **1/1** and the full collaboration-core farm suite at **70/70**.
  Live WebRTC/SIP and sealed DO/LLM evidence remain external.
- Progress (2026-07-25 import-map commit boundary): collaboration import-map
  saves now reject symlinked/non-directory parent components, allocate unique
  0600 create-new temporary files, sync the file and parent directory, and
  atomically replace the final map without following a planted legacy temp
  link. The focused import farm gate is green at **17/17** and the full core
  farm suite at **69/69**; live WebRTC/SIP and sealed DO/LLM evidence remain
  external.
- Progress (2026-07-25 purge-candidate boundary): collaboration purge
  accounting now admits only canonical lower-case 64-character SHA-256
  digests, so signed traversal/mixed-case/malformed references remain inert
  and cannot become filesystem purge candidates. The full core farm suite is
  green at **66/66**; live WebRTC/SIP and sealed DO/LLM evidence remain
  external.
- Progress (2026-07-25 diagnostic projection boundary): collaboration table
  diagnostics now render SQLite values by reference and fail closed at an 8 MiB
  dump budget before hostile text can drive unbounded output allocation. The
  focused projection regression is green at **11/11**, and the full core farm
  suite is green at **67/67**; live WebRTC/SIP and sealed DO/LLM evidence remain
  external.
- Progress (2026-07-25 content-addressed blob boundary): the filesystem blob
  store now accepts only canonical lower-hex SHA-256 paths, rejects traversal
  and final-symlink roots/leaves for reads and purge, and caps both put/get
  materialization at 100 MiB with descriptor-backed reads. The full core farm
  suite is green at **63/63**; live WebRTC/SIP and sealed DO/LLM evidence
  remain external.
- Progress (2026-07-25 bounded import-map state): the durable collaboration
  import map now rejects oversized, non-regular, changing, or invalid-UTF-8
  state before JSON materialization, while preserving missing-map initialization
  and idempotent replay. The full core farm suite is green at **59/59**; live
  WebRTC/SIP and sealed DO/LLM evidence remain external.
- Progress (2026-07-25 replay membership-authority boundary): domain and SQLite
  projection folds now reject a signed `MemberJoined` event that grants a
  different actor membership unless the envelope author is already an Owner;
  self-joins remain valid. The focused core suite is green at **58/58** with a
  dedicated cross-actor replay regression. Live WebRTC/SIP and sealed DO/LLM
  evidence remain external.
- Progress (2026-07-25 collab command-lane authorization): the collaboration
  worker now binds each decoded command's verb to its `action/collab/<verb>`
  topic before applying it, blocking cross-lane capability reuse. The focused
  authorization regression is green at **1/1** and the collab worker module at
  **17/17**; live peer/media/LLM evidence remains external.
- Progress (2026-07-25 mesh media-registry boundary): replicated Nebula bundle
  and media-registry JSON consumers now use bounded no-follow regular-file
  readers that reject final symlinks, special files, oversized or changing
  input, and invalid UTF-8 before peer or shared-account materialization. The
  focused `mesh_media` farm gate is green at **20/20**, and the integrated
  `mackesd --lib` gate is green at **4,083 passed, 0 failed, 1 ignored**; live
  peer/media/LLM evidence remains external.
- Progress (2026-07-25 media-session roaming boundary): replicated per-seat
  playback records now use bounded no-follow regular-file reads rejecting
  special files, final symlinks, oversized or changing input, and invalid
  UTF-8 before JSON materialization while preserving fail-soft seat folding.
  The integrated `mde-media-core --lib` farm gate is green at **234/234**;
  live peer/media/LLM evidence remains external.
- Progress (2026-07-25 bookmark persistence boundary): replicated bookmark
  snapshots, HLC clocks, and append-only segments now use bounded no-follow
  regular-file reads with UTF-8, special-file, symlink, and growth rejection
  before replay or JSON materialization. The integrated bookmark farm gate is
  green at **17/17**. Live peer/media/LLM evidence remains external.
- Progress (2026-07-25 platform passkey persistence boundary): credential
  records, sealed private material, local seal keys, and hardware descriptors
  now use bounded no-follow regular-file reads with hostile-input rejection.
  The integrated Browser passkey farm gate is green at **19/19**. Live
  peer/media/LLM evidence remains external.
- Progress (2026-07-25 Browser policy persistence boundary): managed Browser
  policy documents now use bounded no-follow regular-file reads that reject
  symlinks, special files, oversized or changing input, and invalid UTF-8 while
  retaining the last-good policy on transient corruption. The focused farm gate
  is green at **20/20**. Live peer/media/LLM evidence remains external.
- Progress (2026-07-25 encrypted session and Browser handoff boundaries): the
  sealed KDC session master/session files and Browser latest/outbox records now
  use bounded descriptor-backed regular-file readers that reject final
  symlinks, special files, oversized input, invalid UTF-8, and hostile outbox
  rows before decryption or JSON materialization. Focused farm gates are green
  at **8/8** session-persistence tests and **14/14** Browser session-sync
  tests; integrated gates are green at **104/104** `mde-kdc-host --lib` and
  **110/110** `mde-browser-workers --lib`. Live peer/media evidence remains
  external.
- Progress (2026-07-25 transfer-ledger boundary): transfer ledger and recurring
  sync-pair records now use bounded descriptor-backed regular-file reads with
  final-symlink rejection before JSON parsing, preserving deterministic
  fail-soft loading. Focused farm gates are green at **6/6 ledger tests** and
  **6/6 sync-pair tests**; the integrated BigBoy `mackesd --lib
  --features async-services` gate is green at **4,045 passed, 0 failed, 1
  ignored**. Live peer/media evidence remains external.
- Progress (2026-07-25 persisted Communications boundary): message logs,
  notification preferences, room registries, and presence gossip now load
  through bounded regular-file readers with Unix final-symlink rejection while
  preserving legacy JSON and existing fail-soft defaults. The focused farm
  Communications gate is green at **37/37**; the integrated BigBoy
  `mackesd --lib --features async-services` gate is green at **4,032 passed,
  0 failed, 1 ignored**. Live peer/media evidence remains external.
- Progress (2026-07-24 file-reference boundary): selected collaboration files
  now require regular non-symlink leaves and a bounded 100 MiB read before
  hashing. The focused farm gate is green at **7/7** and the full UI package
  gate at **75/75**; live media/LLM evidence remains external.
- Progress (2026-07-25 conversation timeline boundary): collaboration
  projections now probe one row beyond a 4,096-message timeline limit and bound
  message bodies to 256 KiB before UTF-8 materialization, including the
  single-message lookup path. Deleted tombstones retain an empty body instead
  of surfacing a SQLite NULL. The focused projection regression is green at
  1/1 and the full `.90` `mde-collab-core` library gate at **57/57**; live
  media/LLM evidence remains external.
- Progress (2026-07-24 Documents presentation boundary): document titles,
  summaries, review comments, and Visual Markdown previews are now bounded and
  bidi/control-safe at the egui boundary while Source, Save, and Export retain
  canonical Markdown. The full `.90` `mde-collab-egui` library gate is green at
  73/73; real peer/media evidence remains external.
- Progress (2026-07-24 clipboard publish-boundary hardening): clipboard publish
  input now enforces the 100 MiB transfer/clip ceiling before preview/hash
  command materialization; normal preview behavior is unchanged. The focused
  clipboard gate is green at 5/5 and the full Communications package at 68/68;
  live media/LLM evidence remains external.
- Progress (2026-07-24 Alerts display-boundary hardening): alert headlines,
  sources, structured fields, and action labels are now single-line, bidi/control
  safe, and bounded before egui layout while raw IDs remain unchanged for
  commands. The focused BigBoy Alerts gate is green at 5/5; live media/LLM
  evidence remains external.
- Progress (2026-07-24 replication batch admission hardening): the collaboration
  engine now rejects merge batches above 4,096 envelopes before iteration or
  retention, preserving all-or-nothing replication semantics. Oversized and
  exact-boundary regressions are green, and the full collaboration-core suite
  is green at 55/55 on the farm; live media/LLM evidence remains external.
- Progress (2026-07-24 Communications review/render hardening): the Documents
  strip now emits bounded `RequestReview` and `SubmitReview` commands using the
  current document peers and explicit approve/changes/comment verdicts. Calls,
  Messages, and Files renderers now bound hostile labels/Markdown/file metadata
  before layout while preserving the underlying read-model values and themed
  tooltips. The focused Communications UI farm gate is green at 62/62, the
  collaboration-core gate is green at 53/53, and targeted rustfmt plus the style
  leak gate are green; live media/LLM evidence remains external.
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
- Progress (2026-07-24 document-session materialization hardening): the
  collaboration projection now probes one row beyond a 1,024-session limit,
  caps participant lists at 256 entries, and bounds public titles/participant
  text at 64 KiB before read-model construction. The fail-closed regression is
  green at 1/1 and the full `.90` `mde-collab-core` library gate at 56/56;
  live media/LLM evidence remains external.
- Progress (2026-07-25 clipboard fan-out hardening): `ClearClipboard` now caps
  authored tombstones at the existing 50-entry history budget and rejects an
  oversized aggregate before consuming an event ID or HLC tick. The focused
  BigBoy pipeline gate is green at 6/6; live media/LLM evidence remains external.
- Progress (2026-07-25 KDE Connect fan-out read boundary): request/response
  fan-out rows and replicated row reads now use bounded descriptor-backed
  regular-file reads that reject final symlinks, non-regular leaves, oversized
  or growing input, and invalid UTF-8 before JSON materialization. The focused
  farm gate is green at **10/10**, and the integrated KDC-host library gate is
  green at **92/92**. Live peer/media evidence remains external.
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
- Required outcome: One complete `Mesh Teams` Communications surface replaces
  all seven surfaces without losing existing behavior. Collaboration teams and
  channels become the organizing object, with messaging, documents, files,
  transfers, calls, alerts, clipboard content, local find, basic tasks, Discord
  bridging, and assistive AI sharing one durable, offline-first model. The
  replacement is released only after every surveyed requirement and every
  current-feature parity row is runtime-reachable, tested, and accepted by the
  operator.
- Scope: Full subsystem rewrite; shared collaboration contracts; mesh replication;
  one native egui surface; messaging and threads; document editing and review;
  file management and transfer; alerts; clipboard; voice/video/screen calls; SIP
  interoperability; basic channel tasks; full two-way external Discord server
  bridge; DigitalOcean-hosted LLM assistance; migration; rollback; removal of
  superseded surfaces, workers, crates, state writers, routes, and documentation.
  Recording, transcription, autonomous AI actions, @mentions, priority/urgent
  messages, scheduled messages, emoji/GIF/sticker systems, slash/workflow
  commands, a competing suite-wide omnibox/global Mesh Teams search, per-space
  E2E encryption, partial release, and permanent compatibility shims are out of
  scope.
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
6. Extend the contracts for the Teams + Channels hierarchy, channel meeting
   records, resolve/reopen thread state, shared pinned messages, private saved
   messages, basic channel tasks/action items, and Discord bridge provenance.
   Keep message reactions local-only rather than signed collaboration events.

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
2. Use one Teams-familiar frame built from shared `mde-egui::Style`: a
   Teams-style app rail, a Teams + Channels list, a channel header, contextual
   channel tabs, a rich details pane, and one persistent call bar that survives
   app/channel switches.
3. First entry opens global Activity as the all-Mesh Teams attention inbox. A
   channel opens Posts by default with contextual Files and Calls tabs; direct
   and group messages are channels, not a separate Chat app.
4. Desktop and narrow/tablet layouts keep a fixed split between the rail and
   content. Narrow mode compacts the rail to stable icon-sized geometry instead
   of hiding it. Menus, two-row editor toolbars, tabs, call controls, counters,
   and status areas have bounded dimensions and cannot shift or overlap as state
   changes.
5. Connect Communications entities and actions to existing shell launch/toast
   routes and provide current-channel find. Do not add a global Mesh Teams search
   surface. Notifications use badge counts plus the existing policy-driven toast
   path and route into the exact originating channel and object.

#### Messaging, alerting, and clipboard

1. Every channel has a Markdown-backed conversation timeline, anchored threads,
   and a multi-line rich composer. `Ctrl+Enter` sends, Enter inserts a newline,
   drafts persist locally, delivery state is honest, and edits and deletion are
   accepted only for the author's message during the first five minutes. A later
   attempt remains visible as a denied action, not a silent no-op.
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
   source of truth. The full embedded IDE/editor is the default channel document
   experience; a lighter one-pane document mode may remain available but is not
   the primary entry. Markdown is the only export format; print and preview
   remain available but hidden from the default toolbar.
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

1. Preserve complete local and mesh file-manager parity while presenting channel
   files as transfer-first operational assets: list/grid/details, sorting, hidden
   files, breadcrumbs, editable paths, history, tabs, dual pane, Places/Mesh
   navigation, selection, drag/drop, previews, archives, local search,
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
  4. Markdown messages, rich multi-line composer, composer file/clipboard/
     document create-and-attach, local-only reactions, threads with resolve/
     reopen, five-minute edit/delete, Activity, alert rules, acknowledge/snooze,
     badges/toasts, local channel find, and 100 MB arbitrary-MIME clipboard
     sharing work with real persisted data and explicit failure states.
  5. Full-IDE default document mode satisfies every editor requirement, live CRDT
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
  10. Teams-familiar app rail, Teams + Channels list, channel header tabs, rich
      Details pane, menus, toolbars, call bar, dialogs, and dynamic text render
      without overlap at supported desktop and narrow/tablet viewports.
  11. Migration fixtures are repeatable and rollback-safe, the old/new parity
      ledger has no open rows, forbidden dependencies and private D-Bus names are
      absent, and all superseded runtime code is removed after cutover.
  12. Deterministic rendered screenshots and workflow fixtures cover every
      feature at supported desktop and narrow/tablet sizes; no incomplete,
      disabled, placeholder, or deferred behavior remains. Human visual review
      is informative only and is not a release gate.
  13. Basic channel tasks/action items and full two-way external Discord server
      bridging have tested contracts, clear provenance, loop prevention, and
      honest degraded/offline states.
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
- Progress (2026-07-28 installed-seat gap): the read-only verifier on seat 15
  (`Basement-Test-Workstation`, overlay `10.42.0.5`) found 2 of 11 locked
  default-on overlay topics installed (`nws-alerts` and `usgs-earthquakes`),
  with 9 absent. No Bus publications, provider calls, or live writes were
  performed. The seat is running the older `12.1.1-1` package; current source
  wiring includes the absent worker families, so a current farm RPM and a
  repeat installed-catalog proof are required before this epic can close.
- Progress (2026-07-26 installed overlay catalog verifier): the live-mirror
  verifier now has an installed-seat catalog gate:
  `--require-installed-overlay-catalog` requires `--catalog-overlay-node` and
  fails unless every locked default-on zero-cost
  `state/overlay/<feed>/<node>` topic has a current installed-seat Bus mirror.
  The proof deliberately separates installed-topic presence from live provider
  readiness: explicit status-only mirrors with `availability`/`gaps` can satisfy
  installed presence, but remain `fresh=false` and `ready=false` until a fresh
  `fetched_at_ms` feed snapshot exists; `--require-ready` still requires live
  feed evidence. The gate also fail-closes stale/future feed timestamps and
  stale/future indexed Bus envelope `ts_unix_ms` values, so retained or future
  mirrors cannot prove an installed catalog. `docs/design/maps-live-overlays.md`
  now records this installed-seat proof contract. Verification is green locally
  for `python3 -m py_compile install-helpers/verify-live-mirrors.py`,
  `install-helpers/verify-live-mirrors.py --self-test`, and scoped
  `git diff --check`; farm `.90` slot `probe-key` passed the same Python
  bytecode/self-test after `xcp-build.sh sync`. No Bus publications, external
  feed calls, credentials, or live host mutations were used.
- Progress (2026-07-26 live-overlay verifier no-stale-as-ready hardening):
  `install-helpers/verify-live-mirrors.py` now fails closed on overlay readiness:
  `--require-ready` can no longer report a stale or future-dated `fetched_at_ms`
  mirror as `ready=true` just because the provider availability field is not a
  degraded state. The verifier now separates feed freshness from provider
  availability, returns `fresh=false`, `available=<bool>`, `ready=false`, and
  the raw `availability` string for stale/future snapshots, and its self-test
  covers the stale USGS fixture. `docs/design/maps-live-overlays.md` now records
  this acceptance rule so installed-seat catalog proofs cannot treat aged
  retained mirrors as live data. Focused verification is green on `.90` slot
  `wl-func-012-verifier-ready` after syncing the tree and running
  `python3 -m py_compile install-helpers/verify-live-mirrors.py &&
  install-helpers/verify-live-mirrors.py --self-test`; local
  `install-helpers/lint-worklist.sh --self-test`,
  `install-helpers/lint-worklist.sh docs/platform/WORKLIST.md`, and
  `git diff --check` also pass. No external feeds, paid APIs, credentials,
  synthetic data, or live MG90 motion were used.
- Progress (2026-07-26 AirNow/FIRMS default-on degraded mirrors): AirNow and
  FIRMS now follow the same present-by-default Workstation overlay contract as
  the other zero-cost feeds. AirNow starts unless
  `MDE_OVERLAY_AIRNOW_AQI=0|false|no|off`, publishes an honest unconfigured
  mirror when the sealed key is absent, and keeps prior-location stations out
  of degraded no-fix/failed-refresh output; `docs/design/maps-live-overlays.md`
  now documents the false-y opt-out contract instead of the old opt-in wording.
  FIRMS starts unless explicitly disabled, publishes honest unconfigured/no-fix
  mirrors, withholds prior-location hotspots on failed refresh, and clears
  private stale hotspot state so old thermal detections cannot replay as live.
  Farm evidence is green: `.50` `cargo test -p mackesd --lib --features
  async-services workers::air_quality_overlay::tests -- --nocapture` at
  **10/10**, `.90` `cargo test -p mackesd --lib --features async-services
  workers::firms_overlay::tests -- --nocapture` at **12/12**, `.50`/`.90`
  scoped rustfmt for the touched AirNow/FIRMS worker files, and local
  `git diff --check`. Live AirNow/FIRMS credentials and a fresh MG90 fix remain
  external acceptance evidence, not blockers.
- Progress (2026-07-26 ADS-B/Caltrans/NWS forecast default-on degraded
  mirrors): three more zero-cost Workstation overlay producers now publish
  present retained degraded mirrors instead of leaving catalog topics absent
  when same-host vehicle context is missing. `aircraft_overlay` now starts by
  default unless `MDE_OVERLAY_ADSB_LOL=0|false|no|off`, constructs its blocking
  adsb.lol HTTP client only inside the blocking fetch path, publishes an empty
  licensed `state/overlay/adsb-aircraft/<node>` snapshot for no fresh MG90 fix,
  and clears prior vehicle-scoped aircraft/query origin so stale low-altitude
  tracks cannot replay. `caltrans_camera_overlay` now defaults on when
  `MDE_OVERLAY_CALTRANS_DISTRICT=1..12` is configured unless
  `MDE_OVERLAY_CALTRANS_CAMERAS=0|false|no|off`, publishes an empty licensed
  `state/overlay/caltrans-cameras/<node>` no-fix mirror, clears old
  vehicle-scoped camera rows, and keeps blocking reqwest clients inside
  blocking fetch calls. `nws_forecast_overlay` now defaults on unless
  `MDE_OVERLAY_NWS_FORECAST=0|false|no|off`, publishes a fresh zero-sample
  public-domain degraded `state/overlay/nws-forecast/<node>` mirror for
  no-fix/failed-refresh paths, clears query lat/lon/heading/feed time, and
  drops `last_good` so prior-location forecast rows cannot replay. Focused farm
  evidence is green on the current integrated tree: `.90`
  `cargo test -p mackesd --lib --features async-services
  workers::aircraft_overlay::tests -- --nocapture` at **14/14**, `.170`
  `cargo test -p mackesd --lib --features async-services
  workers::caltrans_camera_overlay::tests -- --nocapture` at **10/10**, `.130`
  `cargo test -p mackesd --lib --features async-services
  workers::nws_forecast_overlay::tests -- --nocapture` at **11/11**, and `.50`
  direct `rustfmt --edition 2021 --check` for the three touched worker files.
  Package-wide `cargo fmt -p mackesd -- --check` still reports unrelated
  pre-existing formatting drift outside these files, so this slice uses scoped
  touched-file formatting evidence. Live `.15` package deployment and catalog
  audit remain follow-up proof before claiming these three topics present on an
  installed seat.
- Progress (2026-07-26 NCDOT/MBTA/NIFC default-on degraded mirrors): three
  additional zero-cost Workstation overlay producers now publish present
  retained degraded mirrors instead of leaving catalog topics absent when
  same-host vehicle context is missing. `traffic_overlay` now starts by default
  unless `MDE_OVERLAY_NCDOT_TRAFFIC=0|false|no|off`, constructs its blocking
  ArcGIS/reqwest client only inside the blocking fetch path, publishes an empty
  licensed `state/overlay/ncdot-traffic/<node>` snapshot for no fresh North
  Carolina vehicle fix, and clears prior vehicle-scoped incident rows/query
  origin so stale NCDOT events cannot replay. `transit_overlay` now starts by
  default unless `MDE_OVERLAY_MBTA_TRANSIT=0|false|no|off`, keeps the MBTA
  blocking client lifetime inside the blocking fetch path, publishes an empty
  licensed `state/overlay/gtfs-transit/<node>` no-fix mirror, and clears
  `last_good` so stale prior-location vehicle rows cannot replay.
  `wildfire_overlay` now starts by default unless
  `MDE_OVERLAY_NIFC_WILDFIRE=0|false|no|off`, keeps the WFIGS blocking client
  lifetime inside the blocking fetch path, publishes an empty licensed
  `state/overlay/nifc-wildfire/<node>` no-fix mirror, and clears stale
  vehicle-scoped fire perimeters. Focused farm evidence is green on the current
  integrated tree: `.90`
  `cargo test -p mackesd --lib --features async-services
  workers::traffic_overlay::tests -- --nocapture` at **13/13**, `.170`
  `cargo test -p mackesd --lib --features async-services
  workers::transit_overlay::tests -- --nocapture` at **16/16**, `.130`
  `cargo test -p mackesd --lib --features async-services
  workers::wildfire_overlay::tests -- --nocapture` at **12/12**, farm
  touched-file rustfmt is green for the three worker files, and local
  `git diff --check -- crates/mesh/mackesd/src/workers/{traffic_overlay.rs,transit_overlay.rs,wildfire_overlay.rs}`
  is clean. Live `.15` package deployment and catalog audit remain follow-up
  proof before claiming these three topics present on an installed seat.
- Progress (2026-07-26 live `.15` Airspace ready-state correction): live
  MG90 root SSH from `.15` was already reachable on `172.20.0.25:2222`, and
  the configured credential-file path worked without exposing the secret. The
  real failure was semantic: the MG90 `wlan0` scan source was reachable, but the
  bounded `iw` scan timed out with `RC=124`, which the parser had treated as
  `NoSource`. `parse_root_ssh_survey` now treats "attempted but no scan
  completed" as a fresh, ready, zero-contact survey with explicit gaps instead
  of reporting "MG90 Wi-Fi survey source unavailable." Focused farm evidence is
  green on `.90` slot `airspace-timeout-ready-final`:
  `cargo test -p mackesd workers::airspace --features async-services
  -- --nocapture` at **13/13**. Fresh Fedora 44 base/browser RPMs from the
  patched tree were staged on `.15`, transaction-tested, installed, and
  manually restarted after the known transient systemd transport-reset hook.
  Post-install proof shows `mde-shell-egui`, `mackesd`, and `nebula` all active
  with `NRestarts=0`, `rpm -V magic-mesh magic-mesh-browser` clean, and
  `/usr/bin/mackesd` sha256
  `78ca9ac16c89cbfea4821fb60fd47113f1b62dad266e79a1ee9fd629d6cbb36a`.
  The live read-only verifier on `.15` passed with `availability=ready`,
  `fresh=true`, `scanner_fresh=true`, `record_count=0`, and the honest gaps
  `Wi-Fi survey failed: wlan0 RC=124`, `MG90 Wi-Fi survey source reachable but
  no Wi-Fi scan completed successfully`, zero observed contacts, Bluetooth
  timeout, and no proven cellular-neighbor command. The installed Airspace
  mirror is therefore no longer `NO SCANNER FEED`; it is ready/fresh with no
  contacts until MG90 produces a successful scan.
- Progress (2026-07-26 MG90 Airspace root-SSH survey adapter): the Airspace
  worker no longer stops at an unconditional production `NoSource` when a seat
  already has `MDE_VEHICLE_GATEWAY` configured. It now wires a production
  `Mg90RootSshSurveyProbe` through the same pinned root-SSH credential path as
  the vehicle worker, runs only a bounded read-only `iw` survey on MG90
  managed/station Wi-Fi interfaces, parses BSS rows into typed
  `AirspaceSurvey` contacts, and publishes `Ready` with honest gaps when the
  scan succeeds but observes zero contacts. The worker still refuses to invent
  cellular-neighbor contacts or Bluetooth RSSI from Status Broadcast or
  metadata-only inquiry output; those remain explicit gaps until a real
  signal-bearing MG90 command/endpoint is proven. `docs/ops/mg90-access.md` and
  the transport-neutral airspace type docs now record this boundary. Live
  read-only MG90 probing from `.15` confirmed the root-SSH plane is reachable,
  `iw`/`iwconfig`/`hcitool` are present, `wlan0` is managed, `wlan1` is AP,
  `aeromon.wlan1` is monitor, and the current bounded scan observes no Wi-Fi
  contacts while GNSS remains `no-fix`/0 satellites; no MG90 configuration was
  changed. Farm evidence is green: `.90` slot `airspace-root-ssh`
  `cargo test -p mackesd workers::airspace --features async-services
  -- --nocapture` at **12/12**, `.170` slot `airspace-types`
  `cargo test -p mackes-mesh-types airspace -- --nocapture` at **4/4**, and
  `.50` slot `airspace-fmt` direct `rustfmt --edition 2021 --check` for the
  touched Airspace files. Live `.15` package deploy/restart remains the next
  proof before claiming the installed Airspace mirror has moved from
  `no_source` to the new ready/offline production state.
- Progress (2026-07-26 IEM/NEXRAD default-on degraded mirror): the
  zero-cost `iem-radar` Workstation worker is now default-on with explicit
  false-y opt-out via `MDE_OVERLAY_IEM_RADAR=0|false|no|off`. When the seat has
  no fresh same-host US vehicle fix, it publishes a present licensed degraded
  `state/overlay/iem-nexrad/<node>` snapshot instead of leaving the catalog
  topic absent, and clears prior vehicle-scoped tiles so an old radar frame is
  not replayed for a stale location. The production HTTP probe now constructs
  its blocking reqwest client inside the blocking fetch path, avoiding Tokio
  runtime-drop hazards. Exact post-integration farm evidence is green on `.90`
  for
  `cargo test -p mackesd --lib --features async-services
  workers::iem_radar_overlay::tests -- --nocapture` at **10/10**, `.170`
  touched-file rustfmt is green, and local `git diff --check` for the touched
  worker is clean. Live radar tiles still require a fresh same-host vehicle
  fix; without that, the correct proof is the honest degraded mirror.
- Progress (2026-07-26 Airspace publication/consumption audit): traced the
  reported Airspace path end-to-end with no code change. `airspace` is a
  Workstation-tier worker in the census (`crates/mesh/mackesd/src/worker_role.rs`
  lines 240-243) and is spawned through `spawn_tiered` from
  `crates/mesh/mackesd/src/bin/mackesd/spawn.rs` lines 1309-1320. Its production
  constructor deliberately has no probe (`crates/mesh/mackesd/src/workers/airspace.rs`
  lines 60-72), and `run` therefore publishes exactly one explicit
  `AirspaceSnapshot::no_source` to `state/airspace/<node>` through the shared
  Bus root (`airspace.rs` lines 307-312 and 144-155). Maps resolves the same
  retained Bus spool via `mde_bus::client_data_dir`, folds the same-node
  `state/airspace/<node>` body through `read_airspace_mirror`, and replaces the
  live-only Airspace state (`crates/desktop/mde-maps-location-egui/src/model.rs`
  lines 731-756, 879-906, 1101-1109, and 1271-1280). The pointer-free render
  path is already present: the mounted Airspace panel binds an egui context and
  schedules a 33 ms repaint heartbeat while visible, and the view clears that
  waker only when Airspace is hidden (`airspace.rs` lines 721-753;
  `view.rs` lines 129-136). A bounded code fix is not clear because the repo
  still has no proven MG90 Wi-Fi/cellular/Bluetooth scanner-contact protocol:
  `docs/ops/mg90-access.md` lines 90-93 explicitly forbids using the documented
  Status Broadcast path to manufacture Airspace contacts. Next implementation
  step is narrow and blocker-free once a real source is identified: add an
  injected `Mg90SurveyProbe` adapter in the Airspace worker for the proven
  endpoint/command, parse it into `AirspaceSurvey`, wire the constructor from
  explicit configuration, and keep the existing `NoSource` fallback for nodes
  without that configured source.
- Progress (2026-07-25 persistent offline map-data root): Maps offline raster
  bundles now default to the persistent RPM/tmpfiles-managed
  `/var/lib/mde/maps` root instead of the installed-seat Bus spool
  `/run/mde-bus/maps`, with `MDE_MAPS_DIR` retained as the exact operator/test
  override and the legacy client-data maps path retained only as a fallback for
  old dev/test checkouts. The Workstation RPM ships
  `/usr/libexec/mackesd/install-offline-map-region`, a bounded helper that
  installs explicit operator-provided MBTiles/gazetteer bundles into the same
  persistent root without downloading or inventing map data; server and
  lighthouse variants intentionally do not ship it. The seat unit pins
  `MDE_MAPS_DIR=/var/lib/mde/maps`, tmpfiles creates the root, `mesh-help`
  exposes status/install commands, and the bootc verifier checks both the unit
  env and tmpfiles entry. Farm evidence is green: `.90` slot `maps-root`
  `cargo test -p mde-maps-location-egui
  basemap::tests::map_roots_use_persistent_var_lib_before_legacy_bus_spool --
  --nocapture` at **1/1**, `.50` slot `maps-pkg`
  `cargo test -p mackesd
  onboard::role_provision::tests::full_rpm_ships_offline_map_installer_and_persistent_map_root
  --features async-services -- --nocapture` at **1/1**, and `.50` touched-file
  rustfmt is green. Local `bash -n`, helper `--self-test`, and `git diff
  --check` are clean. Live map-data installation on `.15` remains a deploy/proof
  follow-up; this slice fixes the packaged location/interface that caused
  installed seats to have no durable map data.
- Progress (2026-07-25 NWS keyless default-on/degraded live proof): the
  `nws_alert_overlay` Workstation-tier worker now starts by default unless
  explicitly disabled with `MDE_OVERLAY_NWS_ALERTS=0/false/no/off`, keeps the
  blocking `reqwest` client lifetime inside the blocking fetch path, and
  publishes an honest public-domain degraded snapshot when a fresh same-host
  MG90 vehicle fix is unavailable instead of staying absent. Farm gates are
  green: `.90` `cargo test -p mackesd --lib
  workers::nws_alert_overlay::tests -- --nocapture` at **15/15**, `.170`
  focused
  `nws_alert_overlay::tests::keyless_nws_alert_producer_defaults_on_with_explicit_false_opt_out`
  at **1/1**, and touched-file rustfmt. After deploying the integrated
  `mackesd` release
  `8bccb4b1d596939942d1a2041256810a225957545f148e1d10279ae6e151ce73` to `.15`,
  the read-only live verifier passed for
  `state/overlay/nws-alerts/Basement-Test-Workstation` with `ok=true`,
  `ready=true`, `fresh=true`, age **968 ms**, `record_count=0`,
  `license_tier=public-domain`, `attribution=NWS`, catalog feed allowed, and
  the explicit gap `fresh same-host MG90 vehicle fix unavailable`. This proves
  default-on keyless NWS alert publication and honest degraded state; it does
  not claim active alert polygons without a fresh MG90/home-location fix.
- Progress (2026-07-25 USGS keyless default-on producer wiring): the
  `earthquake_overlay` Workstation-tier worker is no longer absent-by-default:
  the zero-cost public USGS all-hour GeoJSON producer now starts unless
  explicitly disabled with `MDE_OVERLAY_USGS_EARTHQUAKES=0/false/no/off`, polls
  at the existing 60 s cache cadence, and still publishes only successful
  normalized USGS snapshots to `state/overlay/usgs-earthquakes/<node>`. Farm
  `.50` slot `wl-func-012-usgs` is green for
  `cargo test -p mackesd --lib workers::earthquake_overlay::tests -- --nocapture`
  at **10/10** after moving the blocking reqwest client lifetime inside the
  `spawn_blocking` fetch path; farm `.170` touched-file rustfmt is green, farm
  `.90` Python bytecode + `verify-live-mirrors.py --self-test` is green, and
  `lint-worklist.sh --self-test` is green.
- Progress (2026-07-25 live `.15` USGS absent-to-present proof): after deploying
  the integrated `mackesd` release binary
  `61c0c202f39cb12286e1c69e2b2d74c0c947eecd7124c73dbcf4acd782b4bbaf` to
  `.15`, a read-only verifier streamed over root SSH passed for
  `state/overlay/usgs-earthquakes/Basement-Test-Workstation` with
  `ready=true`, `fresh=true`, `record_count=6`, `license_tier=public-domain`,
  `attribution=USGS`, and age **33.437 s**. The catalog audit now reports
  **present=1, absent=10, invalid=0** for
  `Basement-Test-Workstation`; only `usgs-earthquakes` is present. This proves
  the default-on keyless producer and consumer-proof path for USGS only; the
  other ten zero-cost feeds remain absent and are not claimed complete.
- Progress (2026-07-25 live `.15` zero-cost catalog absence proof): the
  read-only live-mirror verifier now has a node-wide catalog audit
  (`--catalog-overlay-node` plus optional `--require-catalog-complete`) that
  enumerates every locked zero-cost `state/overlay/<feed>/<node>` topic and
  separates absent mirrors from present-but-invalid evidence. Farm `.50` slot
  `wl-func-012-catalog` passed Python bytecode compilation plus
  `verify-live-mirrors.py --self-test`; local diff-check is clean. A read-only
  patched verifier streamed over SSH to live `.15` observed a fresh **383 ms**
  `state/vehicle/Basement-Test-Workstation` mirror with `online=true`, MGOS
  `4.3.0.1`, honest `fix_type=no-fix`, zero satellites, and zero speed, then
  failed the required overlay catalog proof with **present=0, absent=11,
  invalid=0**. Exact absent zero-cost feeds on `.15`:
  `adsb-aircraft`, `airnow-aqi`, `caltrans-cameras`, `firms-hotspots`,
  `gtfs-transit`, `iem-nexrad`, `ncdot-traffic`, `nifc-wildfire`,
  `nws-alerts`, `nws-hourly`, and `usgs-earthquakes`. This collected absence
  evidence only; no external feed was contacted, no MG90 scanner-contact
  protocol or synthetic contact was added, and live painted-overlay/scanner
  proof remains external.
- Progress (2026-07-25 Airspace headless paint-proof seam): the Maps Airspace
  panel now exposes bounded paint stats for proof tooling, counting radar blips,
  live-panel rows, and the honest `NO SCANNER FEED` empty badge without changing
  the production UI entrypoint. A new headless regression folds a fresh Ready
  scanner mirror with Wi-Fi/cellular/Bluetooth contacts and proves one egui
  frame reaches both contact paint paths while suppressing the empty badge; the
  source-less/offline regression also asserts zero contact paint. Farm `.50`
  `mde-maps-location-egui airspace::tests` is green at **15/15**, and
  single-file Airspace rustfmt is green. This is not live MG90 scanner evidence;
  fresh scanner feed and live DRM-seat pixel proof remain external.
- Progress (2026-07-25 MG90 Airspace observation-time boundary): the MG90
  Airspace worker now stamps timestamp-less successful surveys with local poll
  completion time, while future-dated source scan timestamps fail closed to an
  Offline snapshot with zero contacts. The Maps Airspace consumer also rejects
  retained Ready snapshots that lack source-observation time, so timestamp-less
  retained contacts cannot prove live scanner readiness. Farm gates are green at
  **9/9** `mackesd workers::airspace::tests`, **14/14**
  `mde-maps-location-egui airspace::tests`, and touched-file rustfmt is green.
  Fresh live MG90 scanner feed and painted overlay proof remain external.
- Progress (2026-07-25 live-proof verifier catalog/airspace readiness):
  `verify-live-mirrors.py` now validates overlays against the locked zero-cost
  feed catalog and can require fresh `state/airspace/<node>` scanner readiness,
  contact retention, and same-host handoff without mutating the Bus. Farm `.50`
  Python bytecode compilation plus self-test is green, local self-test and
  diff-check are clean. Fresh live provider, MG90 scanner-feed, and painted
  overlay proof remain external.
- Progress (2026-07-25 zero-cost mirror-license audit): the live-mirror proof
  helper now fail-closes overlay evidence whose `license_tier` is missing,
  unknown, non-commercial, paid, trial, personal, educational, research-only, or
  otherwise outside the shipped zero-cost overlay allowlist. The verifier output
  carries `license_tier_allowed`; the `.90` farm gate is green for Python
  bytecode compilation plus `verify-live-mirrors.py --self-test`. Fresh live
  provider, MG90 scanner-feed, and painted-overlay proof remain external.
- Progress (2026-07-25 map popup viewport boundary): the Layers popup now
  clamps offscreen anchors inside non-zero short workspace clips while
  preserving visible checkbox hit targets. The full Maps suite passes
  **233/233**; live provider and MG90 scanner-feed evidence remain external.
- Progress (2026-07-25 gazetteer parent/race boundary): offline Home-address
  geocoding now rejects symlinked or non-directory parent components and opens
  a validated regular database through a stable descriptor-backed SQLite URI,
  with post-open identity checks. The focused geocoder gate is green at
  **10/10**, and the integrated Maps gate is green at **232 passed, 0 failed,
  0 ignored**; live provider and MG90 scanner-feed evidence remain external.
- Progress (2026-07-25 Airspace repaint-liveness boundary): the visible
  Airspace panel now schedules a 33 ms repaint heartbeat and feed folds wake
  the bound egui context without pointer input; leaving the tab clears that
  waker so hidden Airspace cannot keep the seat repainting. Focused Airspace
  tests are green at **13/13** and the full Maps farm suite at **231/231**;
  live MG90 scanner/worker evidence remains external.
- Progress (2026-07-25 stale-route selection boundary): Maps navigation
  now refuses to start guidance when the selected route index is stale or out
  of range, preserving the preview and prior route state instead of claiming
  navigation. Focused regression is green at **1/1** and the full Maps farm
  suite at **231/231**; live provider and MG90 scanner-feed evidence remains
  external.
- Progress (2026-07-25 forecast-retention boundary): NWS forecast retention
  now bounds both samples and periods before they survive into the next frame,
  while preserving stale and unavailable states. The focused forecast gate is
  green at **7/7** and the full Maps farm suite at **229/229**; live provider
  and MG90 scanner-feed evidence remains external.
- Progress (2026-07-25 map consumer input bounds): the earthquake consumer now
  caps retained event vectors before the next frame, while NWS alert polygons
  and vehicle containment reject invalid geographic coordinates and non-finite
  projected points before paint. Focused earthquake/NWS checks are green at
  **9/9** and **8/8**, and the full Maps suite is green at **228/228**; live
  provider and MG90 scanner-feed evidence remains external.
- Progress (2026-07-25 offline geocoder and Airspace observation boundaries):
  offline Home-address geocoding now rejects final symlinks and non-regular
  gazetteer leaves before SQLite access (**9/9** focused tests). Airspace now
  validates the source observation timestamp separately from its publication
  envelope, retracting retained contacts when the scanner frame is stale or
  future-dated (**12/12** focused tests). The full Maps suite is green at
  **225/225**; live provider and MG90 scanner-feed evidence remains external.
- Progress (2026-07-25 future-data boundary): retained IEM radar frames more
  than five seconds ahead of the consumer clock are withheld from paint and
  receive an explicit `invalid future timestamp` badge. The focused IEM slice
  is green at **6/6** and the full Maps suite at **222/222**; live provider and
  MG90 feed evidence remains external.
- Progress (2026-07-25 MG90 Status Broadcast readiness): the vehicle adapter
  now exposes typed local receiver readiness and carries malformed, out-of-range,
  or occupied `MDE_VEHICLE_STATUS_PORT` configuration into the vehicle gap
  snapshot instead of silently disabling the documented beacon plane. The helper
  and vehicle gate is green at **32/32**; the Rev. 6 guide still defines no
  scanner-contact protocol, so Airspace remains honestly `NO SCANNER FEED`.
- Progress (2026-07-25 Advanced Maps viewport regression): Advanced navigation
  rows now reveal inside the actual reserved workspace viewport, and the Layers
  popup is constrained to the workspace clip with an internal scroll region.
  The focused `.50` Maps gate is green at **221/221**; live display proof
  remains external.
- Progress (2026-07-25 Airspace consumer identity boundary): ready Airspace
  contacts now fold duplicate source IDs deterministically before render and
  selection, while preserving the bounded scan and truthful `NO SCANNER FEED`
  states. The focused Airspace gate is green at **13/13**, and the full Maps
  farm suite is green at **230/230**; live provider and MG90 scanner-feed
  evidence remains external.
- Progress (2026-07-25 MG90 protocol audit): the supplied AirLink MG90
  Software Configuration Guide documents UDP Status Broadcast for location,
  GNSS, WAN, VPN, GPIO, ignition, battery, and temperature fields, but does
  not document a Wi-Fi/cellular/Bluetooth scanner-contact endpoint. The
  existing Airspace worker therefore remains wired to the typed probe seam and
  publishes an explicit `NO SCANNER FEED` state until a real scanner protocol
  or configured probe is supplied; no undocumented transport or synthetic
  contacts were added. Source: AirLink MG90 Software Configuration Guide Rev 6,
  pp. 24 and 91, https://www.communica.se/sierrawireless/4118700%20AirLink%20MG90%20Software%20Configuration%20Guide_r6.pdf.
- Progress (2026-07-24 retained-envelope boundary): Maps now reads the
  authoritative on-disk retained envelope through an exact-ULID, bounded 8 MiB
  no-follow path before JSON decoding, while preserving the existing body cap
  and feed-local fail-soft behavior. The model farm gate is green at **60/60**;
  live provider and MG90 scanner evidence remains external.
- Progress (2026-07-25 shared typography adoption): transit and Caltrans camera
  overlay labels/badges now resolve through the shared `TypographyRole::Caption`
  ramp, while technical distance readouts remain monospace at their existing
  size. The final Maps farm suite is green at **215/215**; keyed provider and
  MG90-backed live acceptance remains external evidence.
- Progress (2026-07-24 Airspace publish boundary): the `mackesd` Airspace
  worker now rejects malformed contacts before JSON publication, trims valid
  contact rows to the 64 KiB snapshot bound, and publishes an explicit bounded
  offline state when a snapshot cannot be represented safely. The full BigBoy
  `mackesd` library gate is green at 3,992 passed, 1 ignored.
- Progress (2026-07-24 aircraft label boundary hardening): retained aircraft
  callsigns are sanitized, bounded, width-fit, and viewport-clamped before
  egui layout. The focused aircraft gate is green at 9/9 and the full Maps
  suite at 215/215; live provider-key evidence remains external.
- Progress (2026-07-24 offline-geocoder query boundary): destination-search
  input is capped at 4 KiB before tokenization, FTS expression allocation, or
  SQLite access, while normal prefix search remains unchanged. The focused farm
  geocoder gate is green at 7/7; live provider-key evidence remains external.
- Progress (2026-07-24 map attribution layout hardening): map credits are capped
  at 512 characters with an explicit ellipsis before egui galley layout, while
  the complete normal provider-credit set remains intact. Hostile and normal
  attribution regressions are green, and the full Maps suite is green at
  212/212 on the farm; live provider-key evidence remains external.
- Progress (2026-07-24 AirNow consumer presentation hardening): retained gap
  scanning is capped, pollutant labels are single-line and bidi/control-safe,
  and the alert/status overlays clip to the map viewport with honest paint
  stats. The focused `.50` AirNow suite is green at 13/13; live provider-key
  evidence remains external.
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
- Progress (2026-07-25 NWS alert consumer bound): the Maps warning layer now caps
  retained alerts, polygon rings, and ring vertices before point-in-polygon,
  projection, or triangulation work, validates geographic points, and labels a
  capped mirror honestly. The focused `.90` alert gate is green at 6/6; live
  NWS feed acceptance remains external.
- Progress (2026-07-25 NWS forecast consumer bound): the hourly layer now caps
  retained samples and periods, rejects malformed coordinates/times/ranges, and
  treats future producer timestamps as non-live before projection. The focused
  `.90` forecast gate is green at 7/7; live NWS feed acceptance remains external.
- Progress (2026-07-25 aircraft consumer bound): the adsb.lol layer now caps
  retained tracks before dead reckoning, projection, label layout, or painting,
  preserving valid aircraft within the typed 256-track budget. The focused
  aircraft gate is green at 1/1; live ADS-B acceptance remains external.
- Progress (2026-07-25 FIRMS consumer bound): the thermal-hotspot layer now
  caps retained rows before projection and painting, rejects malformed or
  future-dated coordinates/timestamps, and reports a capped `256+` badge
  honestly. The focused FIRMS gate is green at 8/8 and the integrated Maps
  library gate at 205/205; live FIRMS credentials/fix acceptance remains
  external.
- Progress (2026-07-25 transit consumer bound): the MBTA layer now caps retained
  vehicles before projection and label layout, validates feed/observation time
  and geographic coordinates, and marks future snapshots non-current. The
  focused transit gate is green at 7/7; live MBTA acceptance remains external.
- Progress (2026-07-25 radar consumer bound): the IEM/NEXRAD layer now caps the
  animation history at six frames and four tiles per frame, bounds cached
  textures and decoded RGBA output, and validates tile geometry before decode.
  The focused radar gate is green at 6/6; live radar acceptance remains external.
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

### WL-FUNC-014 - Music interface LAN AirSonic mesh gateway

- Status: Remaining
- Progress (2026-07-26 AirSonic outage metadata/audio cache): `mde-musicd`
  now has a gateway/source-scoped durable cache for cacheable read-only
  AirSonic metadata responses (`getAlbumList2`, artist/search/song/album/genre/
  podcast/radio/lyrics/playlist reads, and starred/frequent-style album
  lists). Successful live responses refresh the cache; temporary transport/
  gateway failures replay the cached metadata without masking API/auth/parse
  failures or playlist mutations. The playback engine also writes complete
  finite `/rest/stream?id=...` responses into the existing recently-played
  audio cache with safe filenames and LRU index updates, and falls back to those
  bytes when a later live stream fetch/read fails; raw radio URLs and cover-art
  fetches stay on their separate paths. Farm evidence is green: `.130` BigBoy
  slot `musicd-airsonic-cache-tests` `cargo test -p mde-musicd --lib --
  --nocapture` passed **116/116**, including one-shot gateway outage metadata
  replay and stream-cache identity tests; `.90` slot `musicd-airsonic-cache-fmt`
  `cargo fmt -p mde-musicd -- --check` passed.
- Progress (2026-07-26 AirSonic gateway proxy responder): `mackesd` now has a
  tiered `media_airsonic_proxy` worker for the Subsonic/AirSonic gateway source
  model. It serves `/mde/airsonic/<source-id>/rest/...` from port 4040 for
  sources whose gateway aliases match the local node, rejects unknown/degraded
  sources, non-REST paths, path escapes, request bodies, and unsafe methods,
  resolves the source's sealed strict `{username,password}` credential through
  `SecretStore`, strips client-supplied Subsonic auth query parameters plus
  auth/hop-by-hop headers, injects fresh server-side `u`/`t`/`s` MD5 token
  auth, and forwards browse/stream/cover-art/playlist REST calls to the LAN
  upstream while preserving range/cache/length headers and streaming bodies.
  Farm evidence is green: `.130` BigBoy slot `airsonic-proxy-hardening` `cargo
  test -p mackesd --lib --features async-services media_airsonic_proxy --
  --nocapture` passed **10/10** including transfer-encoding body rejection, and
  `worker_spawns_and_the_census_do_not_drift` passed **1/1**; `.50` slot
  `airsonic-mackesd-fmt-hardening` touched-file `rustfmt --edition 2021 --config
  skip_children=true --check` passed for the new proxy, spawn, worker census, and
  worker module files.
- Progress (2026-07-26 Music client gateway session hardening):
  `mde-musicd` now treats a WL-FUNC-014 gateway proxy URL as the concrete
  Subsonic session anchor for browse, stream, cover art, and playlist mutation
  endpoints instead of applying the legacy `music.mesh` writer rewrite to pathful
  gateway sources. The rewrite is host-aware now: only an actual `music.mesh`
  authority maps to `music-writer.mesh`; a gateway URL or source id containing
  the text `music.mesh` stays pinned to the selected gateway proxy. Credential
  loading is also strict before any client session is built: `airsonic-creds.json`
  rejects unknown/demo fields and invalid URL/username anchors. Farm evidence is
  green: `.50` `cargo test -p mde-musicd --lib -- --nocapture` passed **112/112**;
  `.90` `cargo fmt -p mde-musicd -- --check` passed.
- Progress (2026-07-26 Music autoconfig gateway credential materialization):
  `music_autoconfig` now prefers manually registered AirSonic gateway sources
  over the legacy shared `media-registry.json` account path. When a gateway
  source exists it selects through the existing healthy/default failover logic,
  resolves the source's sealed `credential_ref` through `SecretStore`, and
  writes only the mesh gateway proxy URL plus sealed username/password into the
  seated user's `airsonic-creds.json`. Secret bodies are constrained to the
  Subsonic auth pair and reject `server_url` override attempts, so credential
  material cannot redirect clients back to a direct LAN URL or `music.mesh`.
  If gateway sources exist but credentials are absent or malformed, the worker
  reports materialization pending and does not silently configure legacy
  `music.mesh`. Farm evidence is green: `.50` `cargo test -p mackesd
  music_autoconfig -- --nocapture` passed **16/16**, `.90` single-file
  `rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/workers/music_autoconfig.rs` passed, and scoped
  `git diff --check` passed. Broad `cargo fmt -p mackesd -- --check` remains
  blocked by unrelated pre-existing formatting drift outside this slice.
- Progress (2026-07-26 gateway source model): `mackesd::mesh_media` now has the
  first durable AirSonic gateway source contract: a replicated
  `airsonic-gateway-registry.json`, validated `AirsonicGatewayRegistration`,
  `GatewayHealth`, client-facing `AirsonicGatewaySource`, canonical LAN upstream
  URL handling, sealed credential references only, explicit rejection of the
  legacy `music.mesh` URL as a gateway upstream, gateway-proxy source URLs,
  upstream dedupe, healthy/default tie-breaks, last-selected healthy source
  selection, and a QNM-Shared plane reader for single or list registry documents.
  Farm evidence is green: `.50` `cargo test -p mackesd mesh_media --
  --nocapture` **28/28**, including the new gateway source tests, and `.170`
  scoped touched-file `rustfmt --edition 2021 --check`.
- Priority: P1
- Complexity: Epic
- Problem: The Music interface's AirSonic/Subsonic method is incomplete and still
  carries older Navidrome / `music.mesh` assumptions. Mesh users need to use an
  AirSonic server located on any mesh node's local LAN from every node in the
  mesh, without requiring each client to be on that LAN.
- Required outcome: A node admin can manually register a LAN-reachable AirSonic
  server on the gateway node; the gateway publishes a mesh-reachable
  proxy/service source; all mesh Music clients can browse and play through that
  source; multiple servers are supported with one mesh default; last-selected
  healthy server wins per user; the same upstream server is deduplicated across
  gateways while gateway health and failover remain visible; and the old
  Navidrome / `music.mesh` path is replaced rather than kept as the primary
  Music model.
- Scope: Native Music interface, `mde-musicd` Subsonic/AirSonic client,
  `mackesd` media registry/autoconfig/service registration, gateway/proxy
  publication, sealed credential materialization, role-gated playback/playlist/
  scan permissions, metadata/audio cache, and live proof helpers. Out of scope:
  routing whole LAN subnets over Nebula, requiring the AirSonic host itself to
  join the mesh, and general AirSonic server administration beyond triggering a
  library scan.
- Relevant files/components: `crates/desktop/mde-music-egui/`,
  `crates/services/mde-musicd/`,
  `crates/mesh/mackesd/src/workers/music_autoconfig.rs`,
  `crates/mesh/mackesd/src/workers/media_registry.rs`,
  `crates/mesh/mackesd/src/mesh_media.rs`,
  `crates/mesh/mackesd/src/workers/app_sync.rs`, and Music/media proof helpers
  under `install-helpers/`.
- Acceptance criteria: manual node-admin registration stores URL plus sealed
  shared read-only credentials; clients materialize credentials through a
  controlled worker; full Subsonic features are available in the Music UI,
  including browse, play, search, playlists, internet radio, podcasts, and scan
  trigger; metadata and recently played audio cache survive temporary AirSonic
  outages; degraded gateway status is visible and failover selects another
  healthy gateway/source when available; access to stream, playlist, and scan
  actions is role/capability gated; and stale Navidrome / `music.mesh`
  assumptions are removed or archived.
- Verification method: focused farm tests for Subsonic client features, gateway
  registry/proxy publication, sealed credential materialization, dedupe/failover,
  role gates, and cache behavior; integrated Music + `mackesd` media tests;
  worklist lint; and live proof where one mesh node proxies a LAN AirSonic server
  for another mesh node.
- Origin or merged source IDs: 2026-07-26 operator AirSonic survey: gateway
  proxy, multiple servers plus default, manual registration, node-admin
  authority, shared read-only sealed credentials, last/default selection,
  degraded failover, deduped gateway health, full Subsonic feature set,
  scan-only admin action, metadata plus recently played audio cache, role-based
  access, replace Navidrome / `music.mesh`, farm plus live LAN proof.

### WL-FUNC-015 - Media Workspace LAN Jellyfin mesh gateway

- Status: Remaining
- Progress (2026-07-26 Jellyfin gateway outage metadata cache):
  `mde-jellyfin` now has a persisted, credential-free `MetadataCache` for
  source-scoped Jellyfin snapshots under `mde/jellyfin/metadata/snapshots.json`.
  The snapshot stores last successful metadata/recent rows, image tags used to
  rebuild cache-stable artwork URLs, and `UserData` resume positions from
  `BaseItemDto`, but strips `MediaSources` before persistence so stream/
  transcode URLs and `api_key` query material cannot become a local credential
  cache. Its write API accepts no token, password, client auth header, or sealed
  `credential_ref`. The Media Workspace mesh-gateway Connect path now requests
  `UserData`, refreshes the snapshot after a live gateway browse, and on
  gateway-client or transport failure restores cached rows with an explicit
  stale/unavailable status that says playback still needs the gateway or a
  downloaded offline title; gateway rows still do not materialize into
  `ServerStore`. Focused farm evidence is green: `.90` slot
  `wl-func-015-metadata-cache` `cargo test -p mde-jellyfin metadata_cache -- --
  nocapture` **2/2**, `.130` BigBoy slot `wl-func-015-media-cache` `cargo test
  -p mde-media-egui mesh_gateway -- --nocapture` **3/3**, `.50` slot
  `wl-func-015-cache-fmt` `cargo fmt -p mde-jellyfin -p mde-media-egui -- --
  check`, and local `git diff --check`. This advances the
  metadata/artwork/recent-playback outage-cache layer without storing
  credentials; live cross-node gateway playback proof remains.
- Progress (2026-07-26 Jellyfin gateway playback role gates):
  `mde-jellyfin` now attaches an explicit `JellyfinAccessPolicy` to clients:
  direct saved-token clients default to `full-access`, `gateway-playback`
  clients can browse, negotiate/download streams, report `/Sessions/Playing*`
  progress, and update user-scoped watched state while denying credential
  exchange before any HTTP request, and `browse-only` clients can render
  metadata/resume rows while denying stream/progress/watched-state actions
  before transport. The Media Workspace gateway client now opts into the
  `gateway-playback` role while still using the non-secret gateway user
  sentinel and not materializing gateway rows into `ServerStore`. Focused farm
  evidence is green: `.90` slot `wl-func-015-jellyfin-role` `cargo test -p
  mde-jellyfin role -- --nocapture` **2/2**, `.130` slot
  `wl-func-015-media-role` `cargo test -p mde-media-egui
  mesh_gateway_jellyfin_client_uses_gateway_playback_role -- --nocapture`
  **1/1**, `.50` slot `wl-func-015-role-fmt` `cargo fmt -p mde-jellyfin -p
  mde-media-egui -- --check`, and local touched-file `git diff --check`.
  This advances the role-gated stream/progress action layer without touching
  the `mackesd` proxy; outage cache behavior and live cross-node playback proof
  remain.
- Progress (2026-07-26 Jellyfin gateway streaming/progress proxy proof):
  `media_jellyfin_proxy` now forwards range-safe upstream response headers
  (`Content-Range`, `Accept-Ranges`, `ETag`, cache validators, and content
  metadata) instead of collapsing streamed replies to only content type/length.
  Focused loopback proxy tests prove a client `Range: bytes=4-9` direct-stream
  request reaches the upstream without client auth/query tokens, upstream
  `206 Partial Content` status, range headers, and body bytes return intact,
  and a `/Sessions/Playing/Progress` POST materializes the sealed gateway user
  into the JSON body, injects server-side auth, strips client auth, and returns
  upstream `204 No Content`. Farm evidence is green: `.90`
  `cargo test -p mackesd --lib --features async-services
  media_jellyfin_proxy -- --nocapture` **12/12**; touched-file rustfmt is green
  on `.50` with `rustup run 1.94.0 rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/workers/media_jellyfin_proxy.rs`; local
  `git diff --check` is clean. Remaining WL-FUNC-015 layers are live
  cross-node proof, role-gated playback actions, and metadata/artwork/recent
  playback cache behavior through outages.
- Progress (2026-07-26 live Jellyfin upstream probe): the operator-supplied
  Jellyfin server at `http://172.20.0.2:8096` is reachable from the dev host and
  from DELL-LAPTOP through the seat network path. Public server info reports
  Jellyfin `10.10.5`, server name `fileserver`, server id
  `91cebca439cd4212a1b82a2fd8ca35ea`, and startup wizard complete. The supplied
  administrator credentials authenticated successfully from both paths without
  printing or storing the access token. A read-only library probe returned three
  views, 50 sampled playable items with 50 media sources, and aggregate counts
  of 2045 movies, 23 series, and 755 episodes. This proves a real upstream is
  available for WL-FUNC-015 follow-on gateway/live-stream testing; it does not
  by itself claim mesh gateway registration, range streaming, or playback-resume
  role gates complete.
- Progress (2026-07-26 gateway Connect/user sentinel): Media Workspace gateway
  rows now have an actionable Connect path for healthy gateway sources that
  carry a sealed `credential_ref`. The controller builds a gateway-scoped
  `JellyfinClient` from the active mesh source without adding anything to the
  local `ServerStore`, using a non-secret placeholder token plus the shared
  `JELLYFIN_GATEWAY_USER_SENTINEL` instead of materializing the real upstream
  Jellyfin user id. The `media_jellyfin_proxy` strips client-supplied Jellyfin
  auth query params before forwarding, injects the server-side
  `Authorization`, and rewrites the sentinel in upstream paths, query strings,
  and UTF-8 request bodies from sealed credential material; missing/invalid
  `user_id` fails as an honest 503. Farm evidence is green: `.170`
  `cargo test -p mackes-mesh-types media_sources -- --nocapture` **1/1**,
  `.90` `cargo test -p mackesd --lib --features async-services
  media_jellyfin_proxy -- --nocapture` **10/10**, `.130`
  `cargo test -p mde-media-egui sources_view_renders_mesh_jellyfin_gateway_rows
  -- --nocapture` **1/1**, and `.130` `cargo test -p mde-media-egui
  mesh_gateway_jellyfin -- --nocapture` **2/2**. A stale `.50`
  `mesh_gateway_jellyfin` run failed before the fixture added `Debug`; it is
  superseded by the green `.130` rerun. Remaining WL-FUNC-015 layers are true
  streaming/range and live cross-node proof, playback-progress/resume role
  gates, and metadata/artwork/recent-playback cache behavior through outages.
- Progress (2026-07-26 gateway range/progress forwarding): the
  `media_jellyfin_proxy` now preserves real streaming response status and cache
  headers from the LAN upstream, including `206 Partial Content`,
  `Content-Range`, `Accept-Ranges`, `ETag`, and the upstream body, while still
  stripping client auth and injecting only the server-side Jellyfin credential.
  Playback progress POSTs also forward JSON bodies/status through the gateway
  and rewrite the client sentinel to the sealed upstream Jellyfin user id. This
  closes the unit-tested streaming/range and progress-forwarding proxy layer;
  live cross-node playback proof and outage cache/resume behavior remain.
- Progress (2026-07-26 Media Workspace source preference/failover): the Media
  Workspace now keeps a user-preferred mesh Jellyfin source outside the local
  `ServerStore`, marks the active mesh route in Sources, and applies the
  WL-FUNC-015 failover order without materializing fake credentials: healthy
  preferred source, healthy mesh default, any healthy source, visible degraded
  default, then first visible degraded row. Degraded rows remain visible with an
  honest unavailable state while Connect stays credential-materialization gated.
  Evidence: `.90` `cargo test -p mde-media-egui mesh_jellyfin_selection -- --
  nocapture` **1/1**, `.50` `cargo test -p mde-media-egui
  sources_view_renders_mesh_jellyfin_gateway_rows -- --nocapture` **1/1**,
  `.170` touched-file `rustfmt --edition 2021 --check`.
- Progress (2026-07-26 Jellyfin gateway proxy responder): `mackesd` now has a
  dedicated `media_jellyfin_proxy` worker, spawned and registered alongside the
  media workers, that serves `/mde/jellyfin/<source-id>/...` for registered
  gateway sources. Gateway source URLs now use a proxy-specific port `8097`
  instead of direct Jellyfin `8096`, avoiding bind collisions with a real local
  Jellyfin server and preventing the descriptor probe from advertising the proxy
  as a direct Jellyfin instance. The worker reads the replicated
  `jellyfin-gateway-registry.json` plane, filters sources to this gateway node,
  rejects unknown/degraded sources honestly, strips the gateway prefix into the
  LAN upstream path/query, resolves the sealed `credential_ref` through
  `SecretStore`, accepts minimal JSON or env-style read-only token bodies, strips
  client auth/hop-by-hop headers, injects server-side Jellyfin
  `Authorization`, and streams upstream responses back without exposing tokens
  to clients. `mde-jellyfin` now has fixture proof that gateway base paths are
  preserved for browse and playback-info request builders. Farm evidence is
  green: `.50` `cargo test -p mackesd --lib --features async-services jellyfin
  -- --nocapture` **21/21**, `.90` `cargo test -p mde-jellyfin
  gateway_base_path -- --nocapture` **2/2**, `.130` `cargo test -p
  mde-media-egui sources -- --nocapture` **12/12**, `.170` `cargo test -p
  mackes-mesh-types media_sources -- --nocapture` **1/1**, `.90` direct
  touched-leaf `rustfmt --check --edition 2021`, and local scoped
  `git diff --check`. Remaining WL-FUNC-015 layers are Media Workspace gateway
  Connect/user binding without leaking Jellyfin user ids, true streaming/range
  and live cross-node proof, playback-progress/resume role gates, and
  metadata/artwork/recent-playback cache behavior through outages.
- Progress (2026-07-26 shared wire + Media Workspace visibility): the
  `state/media/sources` roster now has a shared schema in
  `mackes_mesh_types::media_sources` instead of living only inside `mackesd`,
  covering `MEDIA_SOURCES_TOPIC`, `MediaKind`, `MediaProtocol`, `Reachability`,
  `SourceOrigin`, `MediaSource`, `LaneStatus`, and `MediaSourcesState`. `mackesd`
  imports that shared schema and still publishes the same gateway/direct/mDNS
  roster. The Media Workspace now depends on the shared schema plus a local
  `mde-bus` `Persist` reader, refreshes the retained
  `state/media/sources` record on a coarse cadence, projects Jellyfin gateway
  rows separately from the local Jellyfin `ServerStore`, renders healthy/default
  gateway rows ahead of direct discoveries, keeps degraded gateways visible with
  their reason, shows proxy endpoint/upstream/sealed `credential_ref`, and leaves
  Connect disabled with an honest credential-materialization-pending note so no
  plaintext token or fake saved server is invented. Farm evidence is green:
  `.90` `cargo test -p mackes-mesh-types media_sources -- --nocapture` **1/1**,
  `.50` `cargo test -p mackesd --lib --features async-services media_sources --
  --nocapture` **20/20**, and `.130` `cargo test -p mde-media-egui sources --
  --nocapture` **12/12**. Superseded remaining layer note: the actual gateway
  proxy responder has since landed; Media Workspace Connect/user binding and
  full browse/play/progress proof remain.
- Progress (2026-07-26 `state/media/sources` gateway bridge): `mackesd` now
  lifts replicated Jellyfin gateway records into the generic Media Sources
  roster instead of leaving them only in `mesh_media`. The
  `media_sources` worker reads `jellyfin-gateway-registry.json` from the
  QNM-Shared plane, publishes gateway rows with `SourceOrigin::Gateway`,
  `gateway_node`, canonical `upstream_key`, sealed `credential_ref`, and
  `mesh_default`, keeps degraded gateways visible with an honest reason, sorts
  healthy/default gateway rows ahead of direct discoveries, and dedupes direct
  mDNS rows for the same upstream. The worker now reports an explicit `gateway`
  lane alongside `mesh-registry` and `mdns`. Farm evidence is green on `.50`:
  `cargo test -p mackesd --lib --features async-services media_sources --
  --nocapture` **20/20**, covering gateway projection, default-over-direct mDNS,
  degraded visibility, and QNM-Shared plane folding. The next implementation
  layer remains Media Workspace UI consumption of `state/media/sources`, shared
  wire types, and sealed credential materialization.
- Progress (2026-07-26 gateway source model): `mackesd::mesh_media` now has the
  first durable Jellyfin gateway source contract: a replicated
  `jellyfin-gateway-registry.json`, validated `JellyfinGatewayRegistration`,
  client-facing `JellyfinGatewaySource`, canonical LAN upstream URL handling
  shared with AirSonic, sealed credential/token references only, explicit
  rejection of the legacy `music.mesh` URL as a gateway upstream, gateway-proxy
  source URLs, upstream dedupe, healthy/default tie-breaks, last-selected
  healthy source selection, and a QNM-Shared plane reader for single or list
  registry documents. Farm evidence is green: `.50` `cargo test -p mackesd
  mesh_media -- --nocapture` **35/35**, including the new Jellyfin gateway
  source tests, and `.50` scoped touched-file `rustfmt --edition 2021 --check`.
  The next implementation layer remains `state/media/sources` visibility plus
  Media Workspace gateway rows and sealed credential materialization.
- Priority: P1
- Complexity: Epic
- Problem: The Media Workspace can already model Jellyfin servers, but its live
  path still assumes each server is directly mesh/LAN reachable from the client
  or that each user handles connection state locally. Mesh users need to use a
  Jellyfin server located on any mesh node's local LAN from every node in the
  mesh, without requiring the Jellyfin host itself or every client to join that
  LAN.
- Required outcome: A node admin can manually register a LAN-reachable Jellyfin
  server on the gateway node; the gateway publishes a mesh-reachable
  proxy/service source; the native Media Workspace lists that source as the
  primary Jellyfin path; all mesh Media clients can browse, play, and resume
  through it using sealed shared read-only credentials; multiple servers are
  supported with one mesh default; last-selected healthy server wins per user;
  direct mDNS/mesh-discovered Jellyfin rows are secondary and merge into the same
  source model; and the same upstream server is deduplicated across gateways
  while gateway health and failover remain visible.
- Scope: Native Media Workspace, `mde-jellyfin`, `mackesd` media
  registry/source discovery/service registration, gateway/proxy publication,
  sealed credential materialization, playback/resume state through the shared
  Jellyfin account, metadata/artwork/recent-playback cache, role-gated playback
  actions, and live proof helpers. Out of scope: routing whole LAN subnets over
  Nebula, requiring the Jellyfin host itself to join the mesh, general Jellyfin
  server administration, full offline downloads, and making external Delfin
  launchers the primary surface.
- Relevant files/components: `crates/desktop/mde-media-egui/`,
  `crates/desktop/mde-jellyfin/`,
  `crates/mesh/mackesd/src/workers/media_sources.rs`,
  `crates/mesh/mackesd/src/workers/media_jellyfin_proxy.rs`,
  `crates/mesh/mackesd/src/workers/media_registry.rs`,
  `crates/mesh/mackesd/src/mesh_media.rs`,
  `crates/mesh/mackesd/src/workers/app_sync.rs`, and Media/Jellyfin proof
  helpers under `install-helpers/`.
- Acceptance criteria: manual node-admin registration stores URL plus sealed
  shared read-only Jellyfin credentials/token; clients materialize credentials
  through a controlled worker; the Media Workspace natively lists gateway
  Jellyfin sources ahead of direct discoveries while merging/deduplicating both;
  browse, artwork, play, direct-play/direct-stream/transcode fallback, progress
  reporting, resume, and watched-state updates work through the published source;
  metadata, artwork, and recent playback state survive temporary gateway or
  upstream outages without claiming full offline availability; degraded gateway
  status is visible and failover selects another healthy gateway/source when
  available; last-selected healthy server wins per user with a mesh default
  fallback; access to stream/progress actions is role/capability gated; and no
  library-admin writes are exposed through the shared account.
- Verification method: focused farm tests for Jellyfin gateway registration,
  proxy publication, sealed credential materialization, source merge/dedupe,
  failover/default selection, Media Workspace native source behavior,
  playback-sync permissions, role gates, and cache behavior; integrated
  `mde-media-egui` + `mde-jellyfin` + `mackesd` media tests; worklist lint; and
  live proof where one mesh node proxies a LAN Jellyfin server for another mesh
  node.
- Origin or merged source IDs: 2026-07-26 operator follow-up to mirror
  WL-FUNC-014 for Jellyfin services and the Media Workspace: LAN gateway proxy,
  new sibling epic, sealed shared read-only credentials, last-selected healthy
  defaulting, upstream dedupe across gateways, playback sync/resume allowed,
  metadata/artwork/recent-playback cache, native Media Workspace exposure,
  gateway registrations primary over direct discovery, and farm plus live LAN
  proof.

### WL-FUNC-016 - Native mesh clipboard lanes for seat, browser, and VDI

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: The direct DRM egui seat has no production clipboard provider: key
  handling does not synthesize `egui::Event::{Copy,Cut,Paste}`, platform
  `CopyText` output is ignored, and the remaining clipboard sync path depends
  on `wl-copy` / `wl-paste` even though the governed desktop has no Wayland
  compositor. As a result cut/copy/paste fails locally and cross-app, while
  browser, KDC/mobile, Communications clipboard history, and VDI guest
  clipboard lanes do not share one native mesh contract.
- Required outcome: Clipboard text is a first-class mesh lane across the
  platform. Copying from any local egui app, browser surface, VDI guest, or
  authorized remote/KDC producer updates the active seat clipboard where
  appropriate, publishes the canonical `event/clipboard/clip` body, persists
  into the shared clipboard history, and can be materialized back onto a target
  seat or guest without using Wayland clipboard tools in production.
- Scope: DRM seat clipboard provider, shell-to-mesh clipboard publication,
  `mackesd` clipboard history consumer/responder, Communications Clipboard lane
  actions, browser shell-mediated cut/copy/paste, KDC/mobile inbound
  materialization, and live VDI text clipboard endpoints. Out of scope:
  arbitrary MIME/images, secret filtering, and giving sandboxed browser helpers
  direct host clipboard access.
- Relevant files/components: `crates/shared/mde-egui/src/drm.rs`,
  `crates/desktop/mde-shell-egui/src/`, `crates/mesh/mackesd/src/workers/clipboard_sync.rs`,
  `crates/mesh/mackesd/src/ipc/clipboard.rs`,
  `crates/mesh/mackesd/src/workers/clipboard_bridge.rs`,
  `crates/desktop/mde-collab-egui/src/clipboard.rs`, and the live VDI crates
  under `crates/desktop/mde-vdi-*`.
- Dependencies: Preserve existing `event/clipboard/clip` compatibility with
  body `{ id, text, source, time }`; preserve `action/clipboard/{list,pin,unpin,delete,clear}`;
  preserve VDI clipboard authorization, echo guards, and the 1 MiB guest
  transport cap; coordinate with WL-FUNC-011 / WL-UX-010 so Mesh Teams renders
  the same clipboard lane instead of a parallel store.
- Acceptance criteria: local copy/cut/paste works between egui apps on the DRM
  seat; every seat/browser/VDI/KDC clipboard producer publishes
  `event/clipboard/clip` with stable content id, source, and RFC3339 time;
  clipboard history is updated by consuming that lane rather than watching
  Wayland; a Clipboard row can materialize onto a target seat through an
  authorized action lane; browser clipboard operations remain shell-mediated;
  VNC guest clipboard is bidirectional through real RFB `ClientCutText` and
  `ServerCutText`; RDP/SPICE either use real protocol clipboard channels or
  report explicit unsupported status; duplicate/echo events are debounced.
- Verification method: build-farm tests for `mde-egui` DRM shortcut/output
  handling, shell publication/materialization, `mackesd` lane history folding,
  Communications Clipboard actions, browser mediation, KDC/mobile inbound
  action handling, VNC wire encoding/decoding, VDI host-to-guest and
  guest-to-host flow, worklist lint, and live seat proof on `.15` or Dell
  copying between Editor, Terminal, Browser chrome, Mesh Teams Clipboard, and a
  VDI session.
- Origin or merged source IDs: 2026-07-26 operator report that platform cut and
  paste is impossible, followed by explicit scope lock that all clipboard paths
  must natively connect with the mesh lanes.

## User Interface And Experience

### WL-UX-007 - Car interface (CarPlay-principled vehicle mode)

- Status: Remaining
- Progress (2026-07-26 Car installed-capture freshness guard):
  `install-helpers/verify-shell-pixel-proof.py` now has a `car-screen` profile
  for any Car surface, sharing the same populated left instrument-strip pixel
  guard that `car-home` uses before the home-only dashboard/card/app-strip
  assertions run. Installed Car proof can now add
  `--require-car-instrument-freshness` with a same-run
  `verify-live-mirrors.py --vehicle-node ... --require-online` JSON result; the
  verifier fails closed when that evidence is missing, stale beyond policy,
  offline, not a `state/vehicle/<node>` result, or time-skewed from the PNG
  capture, and reports the mirror topic/host/age/MGOS/fix fields in the metric
  bundle. This intentionally does not OCR the readout values or claim a fresh
  live `.15` capture; it prevents a pixel-only Car PNG from being counted as
  fresh-instrument evidence without a contemporaneous vehicle mirror proof.
  Focused verification is green: local `python3 -m py_compile
  install-helpers/verify-shell-pixel-proof.py` and
  `install-helpers/verify-shell-pixel-proof.py --self-test`; farm `.170` slot
  `ux-car-pixel-fresh` ran the same bytecode compile plus verifier `--self-test`
  clean. Live MG90 drive/fix and physical Car capture remain external gates.
- Progress (2026-07-26 Maps/MG90 Admin advanced-menu viewport regression):
  the Admin section selector no longer uses an unconditional 96 px minimum that
  can place its egui `interact` rect outside a shell-reserved/narrow workspace.
  Each section chip now clamps against the current visible lane, active clip,
  widget max rect, and raw egui screen rect before allocating its hit target.
  The MG90 Setup and Firmware & Recovery card groups now use the existing
  responsive admin-card width helper with wrapped rows, so their two-column
  layouts stack inside narrow Admin viewports instead of squeezing controls
  into off-page-looking columns. Focused farm evidence is green on `.90` slot
  `maps-admin-layout`: `cargo test -p mde-maps-location-egui admin_
  -- --nocapture` at **12/12**, including the new
  `admin_section_strip_hit_targets_clamp_to_tiny_visible_lane` regression that
  click-routes Firmware & Recovery after proving every section target stays
  inside a 72 px visible lane. Local `git diff --check` for the touched
  `view.rs` file is clean. Crate-wide `cargo fmt -p mde-maps-location-egui`
  remains blocked by unrelated pre-existing formatting drift in other Maps
  files, so no broad fmt claim is made. No live `.15` display capture was
  collected in this slice.
- Progress (2026-07-26 Car pixel verifier frame hardening):
  `install-helpers/verify-shell-pixel-proof.py --profile car-home` now verifies
  the full Car frame geometry instead of accepting broad color presence alone:
  populated left driver instrument strip, right-side dashboard cards,
  Ford-blue Navigation cap, and all six bottom app-strip slots. The self-test
  suite now includes fail-closed fixtures for a missing driver strip, missing
  dashboard cards, and missing app strip. The exact final verifier file passes
  local bytecode compilation, `--self-test` (38 s), and `git diff --check`;
  exact post-integration farm `.170` slot `ux-car-pixel-proof` ran bytecode
  compilation plus `--self-test` clean. No live `.15` capture, package deploy,
  MG90 drive/fix, or physical Car proof was collected in this slice.
- Progress (2026-07-26 MG90 Admin single-interface consolidation): Maps &
  Location now exposes one top-level `MG90 Admin` rail target; the former
  Vehicle, Connectivity, Devices & I/O, Location Sources, MG90 Setup, MG90
  Settings, and Firmware & Recovery leaves are internal `AdminSection` pages in
  the requested order with 1-7 keyboard selectors. Car Home routing still sends
  Navigation to Maps `Drive`, while Vehicle opens Admin -> Vehicle. No backend
  MG90 mutation contracts were added. Focused farm verification is green:
  touched-file rustfmt on `.50`, `mde-maps-location-egui admin` on `.90` at
  **11/11**, and BigBoy `.130`
  `car_navigation_and_vehicle_tiles_remain_distinct_maps_routes` at **1/1**.
- Progress (2026-07-25 deterministic live-capture Car pixel verifier):
  `install-helpers/verify-shell-pixel-proof.py` now has a `--profile car-home`
  path for already-captured KMS/linear-GBM PNG artifacts. It fails closed on
  undersized/flat/blank captures and requires the Ford SYNC3 ground, raised
  dashboard/card paint, Ford-blue accent pixels, bottom app-strip/card paint,
  and strong glance text before a Car Home capture can be counted as repeatable
  pixel evidence. Local bytecode compilation and self-test are clean; focused
  farm `.50` slot `ux-pixel-proof` ran bytecode compilation plus `--self-test`
  clean, including generated passing Car and blank-capture failure fixtures. No
  live `.15` capture was collected in this slice; live MG90/physical Car and
  real driving/fix evidence remain external.
- Progress (2026-07-25 Car Home headless pixel proof): `car_home.rs` now has a
  test-only software-rasterized proof that drives the real `car_home_panel`
  through egui `Context::run` plus the existing screenshot backend, then checks
  the 1024x640 canvas for SYNC3 ground/card pixels, the Ford-blue Navigation
  accent cap, dim stale-MG90 vehicle text, and strong live-alert text. BigBoy
  `.130` focused gate is green at **1/1**
  `car_home_pixel_proof_paints_sync3_dashboard_and_honest_mg90_state` (1
  passed, 0 failed, 1,812 filtered out), and `.50` touched-file
  `rustfmt --check crates/desktop/mde-shell-egui/src/car_home.rs` is green. No
  live `.15` proof collection was performed; live MG90/physical Car evidence
  remains external.
- Progress (2026-07-25 MG90 degraded-glance honesty): Car Home now separates the
  vehicle glance label from its live-paint truth. Fresh MG90 telemetry still
  paints live values, while stale/offline/awaiting/simulated MG90 states show
  explicit dim labels such as `MG90 stale`, `MG90 offline`, or `Awaiting MG90`
  instead of being promoted as live telemetry or collapsing to a generic
  descriptor. The BigBoy `mde-shell-egui car_home` gate is green at **14/14**,
  touched-file rustfmt is green, and local diff-check is clean. Live MG90 and
  physical Car evidence remain external.
- Progress (2026-07-25 live `.15` MG90 credential-file migration): the physical
  `.15` seat now uses the preferred `MDE_VEHICLE_ROOT_PW_FILE` service contract
  with `/etc/mackesd/mg90-root-password` owned by root and mode `0600`; the
  legacy plaintext `MDE_VEHICLE_ROOT_PW` environment value was removed from the
  live `mackesd` environment. `mackesd` and `mde-shell-egui` are active with
  `NRestarts=0` after daemon reload/restart, and read-only
  `verify-live-mirrors.py --bus-root /run/mde-bus --vehicle-node
  Basement-Test-Workstation --require-online --max-age-seconds 120` accepted a
  fresh 6.3 s `state/vehicle/Basement-Test-Workstation` mirror with
  `online=true`, MGOS `4.3.0.1`, honest `fix_type=no-fix`, and the existing
  model/OBD gaps. Added the packaged
  `verify-vehicle-credential-hygiene` helper so future live checks fail if the
  legacy env secret returns. Airspace scanner feed, OBD parsing, and physical
  Car/pixel proof remain external.
- Progress (2026-07-25 MG90 Status Broadcast peer boundary): UDP Status
  Broadcast reads now accept packets only from the configured MG90 gateway IP
  before parsing payloads; unexpected senders are dropped in a bounded burst and
  surfaced as an honest status-broadcast gap instead of telemetry. The focused
  farm `mackesd status_beacon` gate is green at **6/6**, and
  `install-helpers/mg90-access.sh inventory` reports `ssh-up lci-up app-up`.
  Live Bus proof was not claimed because `/run/mde-bus` is absent on the dev
  host; physical Car/MG90 proof remains external.
- Progress (2026-07-25 MG90 diagnostic-plane boundary): the vehicle adapter now
  reads only an explicitly configured `/obdii_status/` or `/hdobd_status/`
  MG90 application page, bounds the access contract to those paths, and keeps
  unknown payloads diagnostic-only instead of fabricating typed OBD telemetry.
  Focused vehicle tests pass **35/35**; live MG90 and physical Car evidence
  remain external.
- Progress (2026-07-25 Car keymap persistence boundary): persisted Car
  bindings now walk real parent directories, reject symlinked/special final
  leaves, use private create-new temporary files, and atomically replace the
  target without following a planted legacy temp link. The focused Car-keymap
  gate is green at **10/10**, and the integrated shell gate is green at **1,811
  passed, 0 failed, 0 ignored**. Live MG90 and physical Car evidence remain
  external.
- Progress (2026-07-25 control-only telemetry boundary): Car glance values
  containing only whitespace/control characters now fall back to honest
  no-data labels instead of appearing as live telemetry. The focused Car farm
  suite is green at **12/12** and the integrated shell gate at **1,808/1,808**.
  Live MG90 and physical Car evidence remain external.
- Progress (2026-07-25 Navigation route-priority boundary): Car Home now
  resolves overlapping card activations with explicit Navigation priority, so
  the large blue Navigation action cannot fall through to the Vehicle/OBD
  surface; the independent Vehicle route remains intact. The focused Car Home
  farm gate is green at **12/12**, including the regression. Live MG90 and
  physical Car evidence remain external.
- Progress (2026-07-25 live mirror provenance recheck): read-only `.15`
  verification accepted a fresh `state/vehicle/Basement-Test-Workstation`
  envelope at 5.9 seconds old with `online=true`, MGOS `4.3.0.1`, honest
  `fix_type=no-fix`, zero satellites, and zero speed. The stricter MG90 model
  assertion correctly failed because `general.html` reports no model; the
  retained gaps are `model not reported by general.html` and `OBD not wired
  (OBD-II source is a follow-up)`. No model or fix was inferred.
- Progress (2026-07-25 Car navigation narrow-layout regression): the Car Home
  Navigation tile now has a regression proof at the narrowest supported
  touch-safe layout and fails closed one pixel below either dimension instead
  of exposing a partial or misrouted target. The focused `.90` Car gate is
  green at **12/12**; live MG90 and physical Car evidence remain external.
- Progress (2026-07-25 Car status persistence boundary): the selected Car
  status configuration now uses a descriptor-backed no-follow regular-file
  reader that rejects final symlinks, special files, oversized or changing
  input, and invalid UTF-8 before JSON materialization. The focused Car-status
  farm gate is green at **12/12**; the integrated Maps gate is green at
  **219/219** and the shell gate at **1,797/1,797**. Live MG90 and physical
  Car evidence remain external.
- Progress (2026-07-25 MG90 password-file boundary): the root password input
  now opens without following a final symlink, checks the descriptor's regular
  file/owner/mode, and caps bytes at 4 KiB before UTF-8/string materialization.
  The focused farm vehicle slice is green at **42/42**; live MG90 and physical
  Car evidence remain external.
- Progress (2026-07-25 vehicle diagnostic projection boundary): live vehicle
  mirror adapter gaps are now latest-wins, bounded to 32 entries and 512 bytes
  per diagnostic before Maps renders them, with an explicit capped marker and
  stale-note retraction. The focused `.50` live-mirror slice is green at 4/4;
  live MG90 and physical Car evidence remains external.
- Progress (2026-07-24 Car-keymap persistence boundary): persisted key bindings
  now use a regular-file, 64 KiB bounded read before JSON materialization and
  fail closed for oversized/corrupt data while preserving unknown-key streaming
  behavior. The focused BigBoy keymap gate is green at 9/9; live MG90 evidence
  remains external.
- Progress (2026-07-24 Car-status persistence boundary hardening): persisted
  status selections now use a regular-file, 64 KiB bounded read before JSON
  materialization and fail closed to defaults for oversized/corrupt data. The
  focused regression gate is green at 3/3; live MG90/direct-control evidence
  remains external.
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
- Progress (2026-07-25 Car glance text boundary): route, media, and telematics
  card values are normalized to bounded single-line render text, width-fit with
  visible ellipses, and large alert counts use the honest `999+ alerts` form.
  Existing 44pt targets and no-data fallbacks remain intact; the integrated shell
  Car Home gate is green at 11/11. Live MG90 and physical Car evidence remains
  external.
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

### WL-UX-008 - Workloads app lifecycle redesign

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: The Workloads surface still reads as a stacked delivery-type/panel
  cockpit: delivery types are top-level tabs, resources render as card rosters,
  health and drift are buried in a lens, and typed mutation confirmation is an
  inline arming box. That shape makes provision/plan/run/drift/audit work harder
  to scan than an operator IaC tool should be.
- Required outcome: Workloads becomes a lifecycle-first Construct Ops app:
  Provision opens by default; a native sidebar routes Provision, Plan, Run,
  Drift, Audit, Images, and Containers; delivery types are filters, not top-level
  navigation; the main pane uses dense sortable resource tables with expandable
  rows; a persistent health rail shows backend health, drift, active runs, mirror
  sync, capacity, and latest audit signal; and live/destructive mutation gates
  use a review sheet with exact-echo confirmation.
- Scope: `mde-shell-egui` Workloads/IaC UI state, route layout, provisioning
  form, resource table rendering, health rail, and review-sheet confirmation.
  Preserve existing Bus request/reply verbs, cloud mirror decoding, arming token
  minting, delivery types, workload rows, audit trail, and command contracts.
  Do not revive the superseded OpenStack/Heat design note.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/iac/`
  (`mod.rs`, `menubar.rs`, `placement.rs`, `provision_form.rs`, `status.rs`,
  `images.rs`, `containers.rs`, `views/`, `tests.rs`), shared `mde-egui`
  style/nav primitives only if existing Workloads styling cannot cover the new
  layout, and `install-helpers/verify-workloads-live-proof.py` for live proof
  evidence.
- Acceptance criteria: default entry is Provision; sidebar navigation changes
  lifecycle routes; density toggles compact/comfortable table rows; delivery
  type filters narrow tables without changing route; table sorting is stable;
  expanded rows expose metrics, placement, drift, command/body preview, and row
  actions; Provision renders grouped placement, sizing, image/network, HCL, and
  validation sections with visible plan/provision feedback and sticky actions;
  review-sheet confirmation shows target, command/diff/body, placement,
  blast-radius summary, and requires the exact echo before publishing; desktop
  and tablet-like rendered frames show no overlap, clipping, or hidden health
  rail.
- Verification method: focused `mde-shell-egui` Workloads tests for route
  defaults, sidebar navigation, density, filters/sort, expanded rows, and
  review gating; build-farm `cargo test -p mde-shell-egui iac -- --nocapture`
  plus any broader shell gate required by touched shared UI code; worklist lint
  if this item changes; live or captured Farm + seat proof for desktop and
  tablet-like sizes, with explicit hardware-unavailable note if no seat is
  reachable.
- Progress (2026-07-26 Images lifecycle table + Provision reachability): the
  Images route now renders a dense `Image lifecycle table` before the golden
  image build/promote form. Rows expose Details, Image, SHA256, Active base,
  and Image Actions; expanded rows show placement, promotion state, version,
  full SHA256 content hash, and an `action/cloud/image-build` command preview.
  Candidate rows expose review-gated `Promote…`, while promoted rows show
  `Active base`; promote now requires an explicit version before opening the
  exact-echo review sheet, matching the backend `promote:<name>@<version>`
  contract instead of silently using `latest`. The Provision wide rail was also
  compacted for the Kdam-era text metrics: sticky actions render before the
  HCL/validation rail, and the live-apply validation row is prioritized in the
  compact rail so plan-only/live-apply state remains visible in the desktop
  capture. Evidence is green: BigBoy `.130` slot `images-review-20260726`
  focused `cargo test -p mde-shell-egui images -- --nocapture` passed **9/9**;
  `.90` slot `provision-regress-20260726` focused
  `cargo test -p mde-shell-egui provision_route_ -- --nocapture` passed
  **2/2**; BigBoy `.130` slot `workloads-iac-review`
  `cargo test -p mde-shell-egui iac -- --nocapture` passed **68/68**; `.170`
  slot `ui-slices-fmt-review` touched-file rustfmt passed for the Workloads,
  Communications, and Terminal files changed in this wave. No live seat
  mutation or rendered tablet/live-seat proof was performed.
- Progress (2026-07-26 exact review-sheet echo hardening): the Workloads
  review-sheet confirmation gate now requires the operator echo byte-for-byte.
  Whitespace-padded input such as `  apply ` no longer arms the review sheet and
  cannot mint a capability during the final `perform()` recheck. Evidence: `.90`
  slot `iac-confirm-exact` `cargo test -p mde-shell-egui confirm --
  --nocapture` passed **21/21**; touched-file `.170` rustfmt passed for
  `iac/mod.rs` and `iac/tests.rs`; scoped `git diff --check` passed.
- Progress (2026-07-26 lifecycle-route seam): `crates/desktop/mde-shell-egui/src/iac/`
  now opens Workloads on `Provision` by default, replaces the legacy panel axis
  with `WorkloadsRoute::{Provision, Plan, Run, Drift, Audit, Images, Containers}`,
  renders a native lifecycle sidebar, demotes delivery types to a filter bar,
  adds compact/comfortable density state, surfaces a right-side health rail, and
  renames the destructive inline arming copy to a review sheet while preserving
  the exact-echo Bus gate. `menubar.rs` now opens lifecycle routes instead of
  old panels; delivery-view resource CTAs return to the Provision route. Focused
  tests cover default route/filter/density, Plan filters that do not change the
  active route, every lifecycle route tessellating headlessly, route/filter/density
  state switches, and route icon/label coverage. Evidence: BigBoy farm lane
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=0 install-helpers/xcp-build.sh
  cargo test -p mde-shell-egui iac -- --nocapture` passed 51/51 tests
  (1771 filtered); scoped farm `rustfmt --edition 2021 --check` passed for the
  touched Workloads route/view files.
- Progress (2026-07-26 Plan resource-table seam): the Plan route now renders a
  route-native dense resource table instead of dispatching to the legacy
  per-delivery card-roster modules. `WorkloadsState` owns `WorkloadSort` and an
  expanded-row key; delivery filters still narrow by `DeliveryType`, density
  changes row height, column headers toggle stable ascending/descending sorting,
  rows expose metrics/status/drift/placement, and expanded rows show delivery,
  placement, metrics, drift, mesh reachability, and a command preview that names
  the exact node/target used by the preserved Bus lifecycle action seams. The
  obsolete compiled delivery-view dispatch module/state fields were removed
  rather than carried as a dead compatibility layer. Evidence: BigBoy farm lane
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=0 install-helpers/xcp-build.sh
  cargo test -p mde-shell-egui iac -- --nocapture` passed 54/54 tests
  (1771 filtered), including stable sort, expanded-row keying, and rendered
  expanded command-preview coverage; scoped BigBoy `rustfmt --edition 2021
  --check crates/desktop/mde-shell-egui/src/iac/mod.rs
  crates/desktop/mde-shell-egui/src/iac/tests.rs` passed.
- Progress (2026-07-26 route-persistent placement selector): Run, Images, and
  Containers now render the same placement picker as Provision and preserve the
  shared `selected_node` state across route switches. The prepared route
  mutation seam rejects blank placement before opening a review sheet, and
  focused tests prove Run/Images/Containers do not publish node-agnostic
  `inventory`, `output`, `configure`, `image-build`, or `container-deploy`
  requests when no node is selected. Evidence: BigBoy `.130` slot
  `workloads-placement` `cargo test -p mde-shell-egui iac -- --nocapture`
  passed 58/58 tests (1773 filtered), including
  `run_images_and_containers_share_and_retain_the_placement_selector`,
  `node_scoped_routes_without_selection_emit_no_node_agnostic_requests`, and
  `run_and_prepared_route_actions_fail_closed_without_a_selected_node`; scoped
  `.130` `rustfmt --edition 2021 --check` and local scoped `git diff --check`
  passed for the touched Workloads iac files.
- Progress (2026-07-26 review-sheet facts): prepared route mutations and
  lifecycle mutations now render the frozen review facts before the exact echo
  can publish anything: `action/cloud/*` command, subject, target, placement
  node, request body digest, bounded body summary/preview, and blast-radius
  text. Focused render tests prove the fields are visible for
  `container-deploy` and `instance-delete` while the fixture Bus still has zero
  emitted mutation requests. Evidence: BigBoy `.130` slot `workloads-review`
  `cargo test -p mde-shell-egui review_sheet_renders -- --nocapture` passed 2/2
  tests (1831 filtered); scoped `.130` `rustfmt --edition 2021 --check
  crates/desktop/mde-shell-egui/src/iac/mod.rs
  crates/desktop/mde-shell-egui/src/iac/tests.rs` and local scoped
  `git diff --check` passed for the touched IAC files. Crate-wide
  `cargo fmt -p mde-shell-egui -- --check` remains blocked by unrelated
  pre-existing formatting drift outside `iac/`.
- Progress (2026-07-26 Run/Drift lifecycle table seam): the Run and Drift routes
  now reuse the dense sortable Workloads resource table instead of dropping
  directly into legacy lens-only bodies. A route-specific table mode keeps the
  shared delivery filter, density, sort, expanded-row metrics/placement/drift,
  and exact node/target command preview while changing row actions by route:
  Run exposes lifecycle controls behind the existing review gate, and Drift
  exposes plan-only node actions rather than destructive controls. Evidence:
  BigBoy `.130` slot `workloads-ui-iac` `cargo test -p mde-shell-egui iac --
  --nocapture` passed **62/62** (1775 filtered), including new headless render
  coverage for Run and Drift tables; `.170` touched-file `rustfmt --edition 2021
  --check` and scoped `git diff --check` passed.
- Progress (2026-07-26 Containers/Audit lifecycle table seam): the Containers
  route now renders a dense service-container resource table before the Quadlet
  deploy form, preserving the operator's selected delivery filter while showing
  existing container rows, expanded metrics/placement/drift details, and
  review-gated day-2 row actions. The Audit route now renders the local session
  audit as a dense newest-first table (`Outcome`, `Verb`, `Detail`) instead of
  loose cards, keeping the same honest session-only audit source. Evidence:
  BigBoy `.130` slot `workloads-audit-containers` `cargo test -p
  mde-shell-egui iac -- --nocapture` passed **64/64** (1777 filtered),
  including `containers_route_uses_dense_container_table_before_deploy_form` and
  `audit_route_renders_dense_session_table_newest_first`; `.170` slot
  `workloads-audit-containers-fmt` passed touched-file `rustfmt --edition 2021
  --check crates/desktop/mde-shell-egui/src/iac/mod.rs
  crates/desktop/mde-shell-egui/src/iac/tests.rs`.
- Progress (2026-07-26 Provision viewport reachability): the Provision route now
  keeps grouped placement, identity, sizing, image/network, HCL override,
  validation, and sticky actions reachable inside the desktop-width headless
  viewport. The wide layout uses a compact summary strip and right-side rail for
  HCL/validation/sticky controls; the narrow layout keeps the full vertical HCL
  editor. This fixed the regression where `HCL override` or the sticky action
  buttons fell below the rendered capture after the lifecycle redesign. Evidence:
  `.90` slot `workloads-provision-integrated-r2` `cargo test -p
  mde-shell-egui provision_route -- --nocapture` passed 3/3 tests, including
  `provision_route_renders_grouped_sections_and_sticky_actions` and
  `provision_route_validation_distinguishes_plan_only_nodes`; `.170` slot
  `workloads-provision-fmt-r6` passed touched-file `rustfmt --edition 2021
  --check crates/desktop/mde-shell-egui/src/iac/provision_form.rs
  crates/desktop/mde-shell-egui/src/iac/tests.rs`.
- Origin or merged source IDs: 2026-07-26 planning handoff, "World-Class Infra
  as Code / Workloads Redesign"; UX references named in that handoff: Apple HIG
  sidebars, IBM Carbon data tables/filtering, HCP Terraform workspaces, and
  Kubernetes Dashboard.

### WL-UX-009 - Unified workspace theme and design language

- Status: Remaining
- Progress (2026-07-26 Kdam Thmor Pro platform font standard): the shared
  `mde-egui` font installer now embeds `KdamThmorPro-Regular.ttf` from the
  upstream Google Fonts `ofl/kdamthmorpro` package with its SIL OFL-1.1 license,
  and makes Kdam Thmor Pro the first font for proportional UI copy, named
  `heading` / `nav` families, and Browser chrome. Inter remains embedded only
  as the broad proportional fallback behind Kdam Thmor Pro; IBM Plex Mono and
  Intel One Mono remain the fixed-width code/terminal path. The active design
  authority now names Kdam Thmor Pro as the standard platform UI face. Focused
  verification is green: `.50` slot `kdam-font-egui-20260726` `cargo test -p
  mde-egui fonts -- --nocapture` passed **3/3**, `.90` slot
  `kdam-font-typography-20260726` `cargo test -p mde-egui typography -- --
  nocapture` passed **2/2**, `.170` slot `kdam-font-fmt-r2-20260726`
  touched-file `rustfmt --edition 2021 --check` passed, worklist lint passed,
  and scoped `git diff --check` passed.
- Progress (2026-07-26 themed tooltip style-gate cleanup): the Mesh Teams
  frame, Calls controls, thread resolution button, and local reaction chips now
  route their remaining hover labels through the existing themed
  `CommsHoverExt` / `comms_tooltip` primitive instead of egui's raw
  `.on_hover_text` popup. This clears the current shared style-leak gate's
  hover-text class without changing command behavior or inventing new read
  models. Verification is green: `install-helpers/lint-style-leaks.sh` reports
  zero desktop/shared leaks; `.90` slot `collab-hover-style-test-20260726`
  `cargo test -p mde-collab-egui comms_hover -- --nocapture` passed **1/1**;
  `.170` slot `collab-hover-style-fmt-20260726` touched-file `rustfmt --edition
  2021 --check` passed for `calls.rs`, `messages.rs`, and `frame.rs`.
- Progress (2026-07-26 Quazar Light palette cutover): the shared `mde-egui`
  light color scheme is now production Quazar Light instead of the temporary
  Windows-2000-basic palette. The `Style` module owns named Quazar Light
  ground/surface/border/text/accent/pressed-face tokens, light-mode color
  projection remaps the default accent to the governed Quazar blue, and
  selected/pressed controls keep high-contrast graphite text on the light
  pressed face. Shell Settings copy and tests now call the mode `Quazar Light`,
  and menu/chrome tests resolve the new shared tokens. Focused farm evidence is
  green: `.50` slot `quazar-light-egui` `cargo test -p mde-egui light -- --
  nocapture` **2/2**, `.130` BigBoy slot `quazar-light-system` `cargo test -p
  mde-shell-egui quazar_light -- --nocapture` **1/1**, `.90` slot
  `quazar-light-settings-choice` `cargo test -p mde-shell-egui
  settings_choice_tiles_use_themed_selected_and_hover_colors -- --nocapture`
  **1/1**, `.170` slot `quazar-light-fmt-r2` touched-file `rustfmt --edition
  2021 --check`, plus a stale `WIN2000` / `Windows 2000` grep over non-target
  files and `git diff --check`.
- Progress (2026-07-26 governance/design-lock alignment): `AI_GOVERNANCE.md`
  §4 and `docs/design/platform-interfaces.md` now match the active WL-UX-006 /
  WL-UX-009 operator lock instead of the stale 2026-07-22 dark-only/no-launcher-
  rail/all-icons wording. The authority now names Quazar Dark plus production
  Quazar Light, the persistent Construct Springboard Dock, icon-free
  Bing-wallpaper Home, Browser's governed Material Design 3 exception, Car's
  always-dark AutoSync3 exception, and the focused VDI full-pixel carve-out.
  Verification is green: `install-helpers/lint-doc-supersession.sh`,
  `install-helpers/lint-worklist.sh --self-test`,
  `install-helpers/lint-worklist.sh docs/platform/WORKLIST.md`, targeted stale
  phrase grep across the two authority docs, and scoped `git diff --check`.
- Priority: P1
- Complexity: Epic
- Problem: Construct and Car have active interface epics, and many egui
  workspaces already use pieces of `mde-egui::Style`, `NavigationBar`,
  `Sidebar`, `Sheet`, `Popover`, and shared typography, but there is no single
  worklist owner for a platform-wide theme/design-language pass. The result is
  design drift across shell chrome, workspace frames, Editor/Terminal internal
  chrome, state presentations, motion, icon treatment, and light/dark behavior.
- Required outcome: Every user-facing egui surface reads as one HIG Quazar
  platform: a dense common app frame with shared top bar, sidebar, state views,
  sheets/popovers, tooltips, typography, expressive motion, and icon language;
  Construct keeps the persistent Dock and icon-free Bing-wallpaper Home;
  Browser keeps its governed Material Design 3 exception while sharing quality
  gates; Maps keeps explicitly-marked map-content color exemptions; Car receives
  a full SYNC3/CarPlay-principled pass; and both Quazar Dark and a production
  Quazar Light theme render cleanly.
- Scope: Shared `mde-egui` design primitives, `mde-theme` brand/icon assets,
  `mde-shell-egui` Home/Dock/overlays/switcher/common workspace mount, all
  launchable `mde-*-egui` workspace chrome, and Editor/Terminal internal tabs,
  toolbars, popovers, palettes, sidebars, and status rows. Out of scope:
  changing Bus/control contracts, security/auth behavior, focused VDI
  full-screen pixel reservation, general native-app hosting, full AccessKit
  rollout, and forcing Browser off its governed MD3 local chrome.
- Relevant files/components: `crates/shared/mde-egui/src/` (`style.rs`,
  `motion.rs`, `fonts.rs`, `widgets.rs`, `nav_chrome.rs`, `sheet.rs`,
  `menubar.rs`, `toast.rs`, `capture.rs`, `carbon.rs`),
  `crates/shared/mde-theme/src/brand/icons.rs`,
  `crates/desktop/mde-shell-egui/src/` (`main.rs`, `springboard.rs`,
  `nav_bar.rs`, `backdrop.rs`, `status_bar.rs`, `control_center.rs`,
  `notification_center.rs`, `switcher.rs`, `surfaces.rs`, `car_home.rs`,
  `web/chrome_ui/`, `iac/`), and the embedded workspace crates listed in
  `EMBEDDED_SURFACE_CRATES`.
- Dependencies: Coordinate with WL-UX-006 for Construct Home/Dock authority,
  WL-UX-007 for Car layout and MG90/live-drive evidence, and WL-UX-008 for the
  Workloads lifecycle redesign. Update `AI_GOVERNANCE.md` and
  `docs/design/platform-interfaces.md` where the survey intentionally reverses
  current authority, especially dark-only Construct and the earlier all-icons
  Springboard/no-dock language.
- Acceptance criteria: Quazar Dark and Quazar Light pass palette/contrast,
  shape-remap, screenshot, and no-overlap tests; canonical chrome uses shared
  components unless an explicit exception is documented; empty/loading/stale/
  offline/error/destructive states use shared state components; modal choices
  use shared Sheet/Popover primitives; Editor and Terminal internal chrome use
  the common app frame and shared controls; broad Construct custom glyphs are
  registered, licensed, raster-tested, and cached through the shared icon path;
  expressive motion is centralized and reduced-motion safe; dense tables/lists
  are the default data-heavy body idiom; Browser remains MD3 but aligns with
  shared typography/state/proof standards; Maps UI chrome has no unmarked style
  leaks; and focused VDI keeps full-screen pixels.
- Verification method: build-farm focused and integrated tests for `mde-egui`,
  `mde-theme`, `mde-shell-egui`, and every touched `mde-*-egui` workspace;
  targeted tests for palette resolution, font binding, icon registry/raster
  output, shared frame geometry, state components, sheet/popover behavior,
  Dock/workspace motion, dark/light screenshots, and Car/Construct pixel
  profiles; `install-helpers/lint-style-leaks.sh`,
  `install-helpers/lint-doc-supersession.sh`,
  `install-helpers/lint-worklist.sh --self-test`,
  `install-helpers/lint-worklist.sh`, and local `git diff --check`; live `.15`
  DRM/Sunshine/pixel proof for shell, Car, and representative workspaces when
  hardware is available, with an explicit unavailable-hardware note otherwise.
- Origin or merged source IDs: 2026-07-26 operator 25-question survey:
  all egui surfaces, major redesign, HIG Quazar direction, persistent Dock,
  icon-free Bing-wallpaper Home, Browser MD3 exception kept, full Car pass,
  Editor/Terminal internal chrome included, strict shared-component adoption,
  dark plus Quazar Light, bundled OFL display font, broad custom Construct
  glyph redesign, Dense Ops default, common app frame, sidebar plus top-bar
  navigation, dense tables/lists, expressive motion, visual-polish-only
  accessibility, shared state language, sheets/popovers only, and farm plus live
  proof.

### WL-UX-010 - Mesh Teams near-parity interface redesign

- Status: Remaining
- Progress (2026-07-26 app/channel navigation seam): the `mde-collab-egui`
  frame now has the Teams-style far-left app rail, persistent Teams + Channels
  rail, channel header, and Posts/Files/Calls channel tabs while preserving the
  existing real mode bodies. Activity prefers the cross-space feed when the
  Activity app is active; Teams app hops preserve the selected channel and
  remembered channel tab. Focused farm evidence passed:
  `.90` `cargo test -p mde-collab-egui rail -- --nocapture` = 4/4,
  `.50` `activity_app_prefers_cross_space_feed` = 1/1,
  `.90` `channel_tabs_are_posts_files_calls_only` = 1/1, `.50`
  `set_app_preserves_channel_selection_and_routes_existing_bodies` = 1/1, and
  `.170` scoped `rustfmt --check --config skip_children=true` over the touched
  collab files passed.
- Progress (2026-07-26 rich multiline composer seam): the main channel composer
  and thread reply composer now use multiline text edits, plain Enter retains
  the draft and inserts a newline, and Ctrl+Enter or the Send glyph emits the
  existing real `SendMessage` / `ReplyInThread` commands. Focused evidence:
  `.90` `cargo test -p mde-collab-egui enter -- --nocapture` passed 3/3 tests,
  `.50` `thread_ctrl_enter_emits_reply` passed 1/1, and the warm `.90`
  `cargo test -p mde-collab-egui -- --nocapture` crate gate passed 85/85 tests
  plus doc-tests.
- Progress (2026-07-26 selected-channel Details pane): `mde-collab-egui` now
  reserves a right-side Details pane inside the Mesh Teams frame. The pane reads
  only existing selected-channel read models: directory facts, role, member and
  unread counts, last activity clock, messages, linked files, transfer jobs for
  those file references, document sessions, active calls, and clipboard items.
  Missing projections render as honest zero counts, and stale selections do not
  produce an actionable Details target. Evidence: `.90` slot `collab-details`
  `cargo test -p mde-collab-egui details -- --nocapture` passed 2/2 tests,
  BigBoy `.130` slot `collab-full` `cargo test -p mde-collab-egui --
  --nocapture` passed 87/87 tests plus doc-tests, scoped farm `rustfmt --check
  --config skip_children=true` passed for `frame.rs`, `lib.rs`, and `tests.rs`,
  and scoped `git diff --check` passed. Package-wide `cargo fmt --check` remains
  noisy from unrelated pre-existing import-order drift in other collab modules.
- Progress (2026-07-26 current-channel find): Mesh Teams now has a local
  current-channel Find field in the channel header, keyed per selected channel
  rather than global app state. Posts filtering reads the selected channel's
  retained conversation only, matches visible author/body text case-insensitively,
  does not surface deleted message bodies, reports the exact current-channel
  match count, and emits no Bus/collaboration command. Pure tests cover the
  filter model and deleted-body boundary; headless UI tests cover per-channel
  query persistence and Posts match-count/filter behavior. Evidence: `.90` slot
  `collab-find` `cargo test -p mde-collab-egui channel_find -- --nocapture`
  passed 4/4 tests, `.170` slot `collab-find-model` focused pure model test
  passed 1/1, BigBoy `.130` slot `collab-find-full`
  `cargo test -p mde-collab-egui -- --nocapture` passed 91/91 tests plus
  doc-tests, scoped farm `rustfmt --check --config skip_children=true` passed
  for `lib.rs`, `frame.rs`, `messages.rs`, and `tests.rs`, and scoped
  `git diff --check` passed.
- Progress (2026-07-26 thread resolve/reopen seam): Mesh Teams thread side panes
  now expose actionable Resolve/Reopen controls backed by first-class signed
  `ResolveThread` / `ReopenThread` commands, a convergent `ThreadReopened` event,
  core pipeline admission, and projection foldback into `ThreadTimeline.resolved`.
  The UI still reads the selected thread timeline and emits only command intent
  for the shell Bus route; missing thread projections render non-actionably.
  Evidence: `.50` `cargo test -p mde-collab-types -- --nocapture` passed 37/37
  plus doc-tests; `.170` `cargo test -p mde-collab-core -- --nocapture` passed
  79/79 plus doc-tests; `.90` focused `cargo test -p mde-collab-egui
  thread_resolution -- --nocapture` passed 2/2; BigBoy `.130`
  `cargo test -p mde-collab-egui -- --nocapture` passed 93/93 plus doc-tests;
  `.170` touched-file `rustfmt --edition 2021 --check` and scoped
  `git diff --check` passed.
- Progress (2026-07-26 provider device controls visibility): Mesh Teams now keeps
  microphone, camera, and screen provider controls visible but disabled until a
  real media provider enumerates devices. Calls mode and Settings both render the
  honest `System default` state, explain that enumeration/binding is media-plane
  pending, and avoid fabricated device or Discord provider names. Evidence: `.90`
  `cargo test -p mde-collab-egui provider -- --nocapture` passed 2/2, BigBoy
  `.130` `cargo test -p mde-collab-egui -- --nocapture` passed 95/95 plus
  doc-tests, post-import-order `.130` focused provider recheck passed 2/2,
  `.130` touched-file `rustfmt --edition 2021 --check --config
  skip_children=true`, and scoped `git diff --check` passed.
- Progress (2026-07-26 Discord bridge UI seam): Mesh Teams Settings now renders
  a scrollable, read-only Discord bridge status board, while the selected-channel
  Details pane renders channel-scoped bridge rows. The UI shows honest
  unconfigured, provider-unavailable/degraded, and configured states with
  provenance and two-way Discord/Mesh flow status; it emits no commands, calls no
  Discord provider, and does not invent server names. Evidence: `.90` slot
  `discord-ui-rerun` focused `cargo test -p mde-collab-egui discord_bridge --
  --nocapture` passed **2/2**; `.50` slot `discord-types-rerun` focused `cargo
  test -p mde-collab-types discord_bridge -- --nocapture` passed **1/1**;
  `.170` slot `discord-fmt-rerun` touched-file `rustfmt --edition 2021 --check
  --config skip_children=true` passed.
- Progress (2026-07-26 local-only quick reactions): Mesh Teams Posts now expose
  a constrained local reaction strip (`Ack`, `Check`, `Watch`) as seat-local
  view state keyed by message event id. Clicking the current chip clears it;
  clicking another chip replaces it; no `CommandSink` entry, collaboration
  command, signed event, or mesh-visible reaction is emitted. This preserves the
  WL-UX-010 scope boundary that emoji/GIF/sticker systems remain out of scope.
  Evidence: `.50` slot `collab-local-reactions` focused
  `cargo test -p mde-collab-egui local_reaction -- --nocapture` passed
  **2/2**; `.170` slot `collab-full-after-reactions`
  `cargo test -p mde-collab-egui -- --nocapture` passed **98/98** plus
  doc-tests; touched-file `.170` rustfmt and scoped `git diff --check` passed.
- Progress (2026-07-26 focused first-open activity fold): the shell-side
  Communications data fold now reads heavy per-space Mesh Teams mirrors only
  for the focused channel on first open and on channel switch, while keeping the
  directory/global rollups live. The Activity `All` filter also uses the source
  slice directly instead of allocating a per-row filtered vector before
  `ScrollArea::show_rows` virtualizes paint. This targets the seat `.15`
  slowdown when opening Mesh Teams Activity without changing collaboration Bus
  topics or command contracts.
- Progress (2026-07-26 seat `.15` Activity read-bound clamp): the shell-side
  Mesh Teams mount now defensively clamps retained Activity Bus mirrors to the
  newest 1,024 rows before exposing them to the egui surface, so older or stale
  live-seat mirrors cannot reintroduce an unbounded feed. Unread counting now
  walks newest-last rows only until the durable cursor, and the per-frame
  mark-read path advances from the newest retained row instead of scanning the
  whole feed for a max clock. Evidence: BigBoy `.130` slot
  `ux010-activity-clamp` focused
  `cargo test -p mde-shell-egui
  focused_activity_feed_is_clamped_and_read_cursor_uses_newest_row --
  --nocapture` passed **1/1**; BigBoy `.130` slot
  `ux010-activity-communications`
  `cargo test -p mde-shell-egui communications::tests:: -- --nocapture` passed
  **7/7**; `.170` slot `ux010-activity-fmt-file` synced checkout
  `rustfmt --edition 2021 --check
  crates/desktop/mde-shell-egui/src/communications/mod.rs` passed. A
  package-wide `cargo fmt -p mde-shell-egui -- --check` remains blocked by
  unrelated pre-existing formatting drift in other shell files outside this
  slice.
- Progress (2026-07-26 pins/saved pending seam): Mesh Teams Posts now paint a
  per-message `Keep` row with visible `Pin` and `Save` controls. Because
  `MessageView` has no message-level pinned/saved fields and `CollabCommand`
  has no pin/save-message verbs, the controls are disabled with explicit
  pending-read-model copy and take no `CommandSink`, so no shared pin or private
  saved-message state is fabricated. Evidence: `.50` slot `collab-pin-save`
  `cargo test -p mde-collab-egui message_pin_save_affordances -- --nocapture`
  passed 1/1 after the final source sync; `.90` slot `collab-pin-save-fmt`
  touched-file `rustfmt --edition 2021 --check --config skip_children=true`
  passed for `messages.rs` and `tests.rs`. A crate-wide
  `cargo fmt -p mde-collab-egui -- --check` was not used as evidence because it
  still reports unrelated pre-existing import-order drift in `clipboard.rs` and
  `documents.rs`.
- Priority: P0
- Complexity: Epic
- Problem: The live Communications surface has a strong Bus-backed foundation,
  but the interface is still a fixed spaces rail plus eight primary tabs and a
  bottom call bar. That shape does not match the operator's requested
  Microsoft-Teams-like product model, makes the capability set feel fragmented,
  exposes utility modes too prominently, and leaves composer, channel, meeting,
  file, document, task, Discord, and car-mode workflows below the requested
  near-parity bar.
- Required outcome: `Mesh Teams` presents a Teams-familiar, operator-focused
  workspace: app rail, Teams + Channels list, channel header, Posts/Files/Calls
  tabs, global Activity inbox, rich Details pane, multi-line rich composer,
  side-thread pane, pinned/saved message affordances, ad-hoc channel meetings,
  transfer-first files, full-IDE document collaboration, basic tasks, contextual
  Clipboard/Transfers, full two-way Discord bridge status, and glance-safe Car
  Mode. The visual language should read very similar to Teams while using the
  governed Quazar/Construct tokens and shared components.
- Scope: `mde-collab-egui` frame/navigation/body renderers, message composer,
  thread UI, call/device/meeting UI, files/transfers/clipboard panels, document
  entry points, task panels, Discord bridge surfaces, Car Mode Communications
  treatment, and the shell Communications mount/launcher/toast routing needed to
  expose the redesigned experience. Out of scope: recordings/transcripts,
  @mentions, message priority/urgent labels, scheduled messages, global Mesh
  Teams search, emoji/GIF/sticker systems, slash/workflow commands, and a generic
  Teams app/bot platform beyond the explicit Discord bridge.
- Relevant files/components: `crates/desktop/mde-collab-egui/`,
  `crates/desktop/mde-shell-egui/src/communications/`,
  `crates/desktop/mde-shell-egui/src/surfaces.rs`, shared `mde-egui`
  frame/nav/sheet/popover/tooltip/style primitives, `mde-collab-types` read
  models and commands owned by WL-FUNC-011, `mde-collab-core` projections,
  `mackesd` collaboration and future Discord bridge workers, and Car
  Communications routes.
- Dependencies: Coordinate with WL-FUNC-011 for backing contracts, worker
  behavior, media, coauthoring, tasks, and Discord bridge semantics; WL-UX-009
  for shared Quazar/Construct design language; and WL-UX-007 for glance-safe Car
  constraints.
- Acceptance criteria: the eight-tab layout is replaced by an app rail plus
  Teams + Channels list; direct/group conversations render as channels; global
  Activity shows unread and alert quick filters; Alerts, Transfers, and Clipboard
  are both rail-reachable and contextual where appropriate; channels default to
  Posts/Files/Calls tabs with a rich Details pane; composer supports rich
  formatting, multiline editing, `Ctrl+Enter` send, file attachment, clipboard
  attach, and document create/attach; reactions are local-only; current-channel
  find works without a global search surface; threads resolve/reopen in a side
  pane; shared pins and private saved messages are reachable; ad-hoc meetings
  create persistent channel context; device controls are disabled-but-visible
  until real providers enumerate; screen share can escalate to remote control;
  files read as transfer-first; Documents opens to the full IDE/editor and live
  coauthoring state; basic tasks/action items are usable in channel context;
  Discord bridge UI shows two-way status, provenance, and degraded states; Car
  Mode exposes only glance-safe alerts and calls; excluded features are absent.
- Verification method: build-farm focused tests for `mde-collab-egui` route
  state, channel hierarchy rendering, composer shortcuts/actions, details pane,
  threads, pins/saved messages, local find, call/device states, tasks, Discord
  bridge UI, contextual utility panels, and Car Mode limits; contract/projection
  tests in WL-FUNC-011 for any new read models or commands; `mde-shell-egui`
  launcher/toast/navigation tests; `install-helpers/lint-style-leaks.sh`,
  `install-helpers/lint-worklist.sh --self-test`,
  `install-helpers/lint-worklist.sh`, and `git diff --check`; live or captured
  DRM/Sunshine proof for desktop, narrow/tablet, and Car profiles when hardware
  is reachable, with an explicit unavailable-hardware note otherwise.
- Origin or merged source IDs: 2026-07-26 operator 50-question Mesh Teams survey:
  near Teams parity, mesh operators, `Mesh Teams` label, very Teams-like visual
  model, single big push, Teams + Channels hierarchy, direct/group messages as
  channels, Teams-style app rail, favorites/pins only, global-only Activity,
  Alerts folded into Activity, contextual Clipboard/Transfers, rich Details pane,
  Posts/Files/Calls channel tabs, operator rail set, rich multi-line composer,
  `Ctrl+Enter` send, attachments, clipboard/document composer integration,
  local-only reactions, no @mentions, no priority/scheduled messages, no global
  search, current-channel find, unread/alerts quick filters, no read receipts,
  resolve/reopen side threads, pinned plus saved messages, no slash commands, no
  emoji/GIF/stickers, ad-hoc meetings only, channel Posts as meeting discussion,
  no recording/transcription, WebRTC P2P first, disabled provider/device controls
  until real enumeration, one share flow with remote-control escalation, 2-20
  participant target, transfer-first files, full IDE default documents, required
  live coauthoring, basic tasks, full two-way external Discord server
  integration, glance-safe Car Mode, and worklist placement in both WL-FUNC-011
  and this linked UX epic.

### WL-UX-011 - Unified This Node hardware center

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Local-node controls are fragmented across This Node, System,
  Storage, Device Manager, About, the status bar, and Control Center. The
  existing Bluetooth, display, power, and input pages have useful foundations,
  but workstation Wi-Fi is absent, keyboard backlight has no backend or GUI,
  the detailed sound page is read-only, and tap-to-click is exposed without a
  real direct-seat apply path. The shell also lacks one coherent laptop and
  hardware-management experience for batteries, thermals, docks, firmware,
  privacy devices, and safe manufacturer controls.
- Required outcome: `This Node` is the one searchable, HIG-principled,
  progressively disclosed hardware center for the local machine. Its grouped
  sidebar exposes Overview; Connectivity (Wi-Fi & Ethernet, Cellular, Hotspot,
  VPN, DNS & Proxy, Bluetooth); Display & Sound (Displays & LCD, Sound, Camera
  & Privacy); Input (Keyboard & Backlight, Mouse & Touch, Pen & Gestures);
  Power & Performance (Power & Battery, Thermals & Fans, CPU & GPU); Hardware
  (Devices & Drivers, Firmware & Docking, Storage); Personalization (Appearance
  & Wallpaper); and Mesh & System (Identity & Role, Mesh Pairing & Network,
  Remote Proofing, About). Every page remains discoverable on unsupported
  hardware and shows an honest unavailable or degraded state instead of
  disappearing or fabricating data.
- Scope: Consolidate the durable local-node destinations currently exposed by
  This Node/System, Storage, Device Manager, About, and Surface enablement into
  the one This Node route, while preserving legacy deep links by normalizing
  them to the corresponding page. Implement local-only control and telemetry
  for NetworkManager/ModemManager connectivity; BlueZ; displays and LCD/DDC
  brightness; PipeWire/WirePlumber sound; keyboard, pointer, touch, pen, and
  gesture devices; UPower/logind/power profiles; thermals, fans, CPU/GPU
  profiles; devices, drivers, firmware, docks, storage, and supported OEM
  controls. Keep Control Center as a transient quick-controls overlay and the
  status bar as glanceable chrome, with no second durable settings hierarchy.
  Remote hardware mutation, generic arbitrary sysfs/path writes, raw MSR/SMI or
  `/dev/mem` access, lock/PAM replacement, and a host application ecosystem are
  out of scope.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/system/`,
  `crates/desktop/mde-shell-egui/src/device_manager/`,
  `crates/desktop/mde-shell-egui/src/storage/`,
  `crates/desktop/mde-shell-egui/src/about.rs`,
  `crates/desktop/mde-shell-egui/src/control_center.rs`,
  `crates/desktop/mde-shell-egui/src/status_bar.rs`,
  `crates/desktop/mde-shell-egui/src/hotkeys.rs`,
  `crates/desktop/mde-shell-egui/src/surfaces.rs`,
  `crates/desktop/mde-shell-egui/src/seat_pump.rs`,
  `crates/desktop/mde-seat/`, direct DRM/libinput support in
  `crates/shared/mde-egui/`, typed local-control contracts in
  `crates/mesh/mackes-mesh-types/`, `mackesd` hardware/firmware workers under
  `crates/mesh/mackesd/src/`, RPM/package service dependencies, and the current
  interface/host-control design notes under `docs/design/`.
- Acceptance criteria: one public This Node route owns all durable local
  settings, diagnostics, and storage/about/device views; the sidebar hierarchy,
  search results, compact/narrow layout, advanced disclosures, and legacy-route
  normalization are covered by tests. Wi-Fi supports radio state, scan,
  WPA2/WPA3 and hidden/802.1X joins, saved-network priority/forget, and honest
  signal/security state; Ethernet, cellular/APN, hotspot, DNS, proxy, and
  imported WireGuard/OpenVPN activation are functional when their system
  providers exist. Connectivity changes preserve `nebula1`, mesh DNS, overlay
  routes, and lighthouse reachability, warn before disconnecting the sole
  uplink or taking it over for a hotspot, handle credentials through an
  in-process SecretAgent, and never place credentials in Bus payloads, logs,
  shell arguments, or serialized GUI state. Bluetooth retains adapter
  power/discoverability/pairability, scan, PIN handling, pair, connect, trust,
  and forget behavior. Local state and changes use typed
  `SeatSnapshot`/`SeatEvent` and hardware-control contracts rather than direct
  shell writes from the GUI. Displays support enablement, mode, refresh,
  arrangement, scale/rotation, and internal/DDC brightness; keyboard-backlight
  devices support level changes, multiple-device selection, brightness hotkeys,
  and OSD feedback. Sound exposes selectable/default outputs and inputs, ports,
  profiles, application/VM/mesh strips, mute, volume, solo, and real peak
  levels through a long-lived PipeWire adapter, with an explicitly labeled
  no-meter fallback when only compatibility tools are available. Mouse, touch,
  keyboard, pen, and gesture policy is per-device and applied through the real
  udev/libinput direct-seat path, including functional tap-to-click rather than
  a presentation-only toggle. Control Center offers the safe quick subset for
  connectivity, Bluetooth, sound, LCD/keyboard brightness, and power; status
  chrome distinguishes underlay connectivity from mesh state and adds numeric
  battery plus microphone/camera privacy indicators. Camera and microphone
  devices enumerate with real privacy/policy state; fingerprint capability is
  diagnostic-only and does not change the existing curtain or PAM boundary.
  Battery pages show charge, health, source, time estimates, charge limits,
  profiles, idle/lid behavior, and supported sleep/power actions; thermals,
  fans, CPU/GPU, docks, devices, drivers, firmware, and storage use live typed
  state. A privileged hardware-control worker accepts only explicit action
  classes for platform profile, fan mode/curve, bounded CPU power limits, GPU
  profile, device enablement, and Thunderbolt authorization. Standard kernel
  interfaces plus Microsoft Surface, Dell, Lenovo, HP, and ASUS adapters are
  capability-detected; unsupported or unsafe controls stay visible but disabled
  with a reason. Manufacturer writes are bounded, armed/audited, thermally
  constrained, watchdog-protected, and automatically return to a safe profile.
  Existing fwupd and Surface enablement behavior is generalized without
  regressing Surface-specific support, and all actions remain node-local.
- Verification method: fixture and contract tests cover This Node routing and
  search, progressive disclosure, unavailable/degraded states, NetworkManager,
  ModemManager, BlueZ, PipeWire, UPower/logind, fwupd, libinput, sysfs/backlight,
  DDC, hwmon, docks, and each OEM capability adapter. Run focused
  `mde-shell-egui`, `mde-seat`, `mackes-mesh-types`, and `mackesd` tests plus
  workspace build/clippy/fmt gates on the build farm, placing the longest shell
  or workspace job on BigBoy; run worklist, style, architecture-boundary, secret,
  and documentation-supersession lints. Render dark, light, narrow, large-text,
  unsupported-hardware, and destructive-confirmation states. Complete final
  direct-DRM/Sunshine captures and physical control proofs on available
  workstation/laptop hardware for connectivity, audio I/O, LCD and keyboard
  brightness, mouse/touch, battery/power, firmware/dock, and one safe control
  for each reachable OEM; record explicit hardware-unavailable evidence for
  capabilities that cannot be exercised.
- Origin or merged source IDs: 2026-07-26 local-node GUI audit and operator
  decisions: close gaps plus polish; full connectivity including hotspot,
  proxy, DNS, and VPN; one large This Node hardware center; progressive
  disclosure; full laptop depth; OEM writes; first-class Microsoft Surface,
  Dell, Lenovo, HP, and ASUS support. Coordinates with WL-UX-006 for Construct
  shell chrome and with WL-UX-009 for the shared Quazar/HIG design language
  without creating a competing interface workstream.

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
