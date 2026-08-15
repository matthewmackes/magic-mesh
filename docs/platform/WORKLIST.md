# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-15 executable story rewrite

- **3 active epics:** 3 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
- **Execution order:** complete ARCH-010 stories in order; then consume its
  contracts in ARCH-008, ARCH-009, FUNC-019, FUNC-018, and FUNC-020. Run the
  vertical slices FUNC-011/FUNC-016, FUNC-017, FUNC-021, and FUNC-022 next. Integrate
  UX-009, UX-011, UX-012, and UX-014 at their named story gates. Close
  every slice through CRIT-006 and CRIT-007.
- **Single-authority lock:** typed Workload operations are the only VM/container
  lifecycle API; mackesd is the only daemon authority; mde-bus is the only
  platform bus; the shell renders typed bounded projections and sends typed
  intent. Do not add a compatibility shim, parallel tracker, direct backend
  call, raw command, or GUI-owned service state.
- **Product lock:** Construct is one egui DRM thin-client shell. Native apps run
  in governed VMs or approved native collaboration/media surfaces. There is no
  Wayland compositor, host Browser engine, OpenStack control plane, or retired
  LizardFS/cloud-hypervisor path.
- **Evidence lock:** a story is incomplete until its deliverable, focused hostile
  tests, farm command and result, and required live/package evidence are recorded
  in docs/platform/evidence/. Missing hardware or provider access is a named
  blocker, not a passing substitute.
- **Shared release-proof ownership:** first-release input admission, signed
  artifact/package proof, installed baseline acceptance, corrected-forward
  recovery, and deferred provider/live proofs are owned by `WL-TEST-002`.
  Product epics must not duplicate those rollout tasks; they retain only
  product-specific implementation and integration gaps, and cite `WL-TEST-002`
  when its acceptance is a dependency.
- **Test-seat cap (operator lock 2026-08-14):** no validation, rollout proof,
  capture, chaos, recovery, or acceptance activity may require or exercise more
  than two physical test seats. An epic may substitute named seats when its
  hardware is the subject, but must remain at two or fewer. Historical three- and
  five-seat evidence stays factual but creates no forward multi-seat requirement.
  Lighthouses are not test seats and retain their independently governed quorum
  proof.
- **Privacy-retention lock (operator lock 2026-08-10):** system logs, Bus
  history, transfer ledgers, collaboration JSONL, application histories, and
  audit records have a fleet-wide maximum lifetime of six hours. No priority or
  audit class is exempt. Configuration, identities, credentials, current
  materialized state, user media, queued payloads, and VM disks are not history
  and must survive each epoch. Offline replicas must not be able to restore an
  expired record after they rejoin. The release-33 Dell churn, compact-Bus,
  synchronized privacy epoch, VM CPU/I/O profile, and exact cold-boot proof is
  recorded in `docs/platform/evidence/WL-ARCH-008-WL-ARCH-009-WL-ARCH-010-WL-CRIT-007-2026-08-10-dell-churn-release33-r134.md`.
- **Farm lock:** heavy verification is farm-only; route the longest job to
  BigBoy at 172.20.0.130, use explicit MCNF_BUILD_HOST and MCNF_BUILD_SLOT, and
  never run filler tests.
- **Rollout lock:** prove each release activity on no more than two selected
  physical seats and the independently required lighthouses. Wider fleet
  deployment, when needed, proceeds in separately bounded waves and does not
  expand the test requirement. Publish the red AI-GENERATED-ALERT and wait five
  seconds before each seat mutation. Recover failures by re-enrollment and
  corrected-forward deployment, never rollback.
- **Story format:** execute stories top-to-bottom. Do not start a story until
  every dependency is green. If a dependency or external resource is absent,
  set the epic to Blocked with the exact missing item; do not invent evidence.

## Active Drain Goal

Finish the Music and Media Player vertical slice (daemon catalog/playback,
mpv frame/audio, library/Jellyfin, cache/offline, discovery, casting, handoff,
visual proof) while preserving the single Workload, Bus, and typed executor
authorities. Archive old MEDIA and FUNC-007 IDs; they are evidence only.

## Service Release Queue

1. Workloads runtime and native attachment.
2. Browser VM cutover and standalone legacy repository.
3. Process-isolated mackesd and Workers.
4. Collaboration Suite and rich clipboard.
5. Maps/MG90 and universal resource browser.
6. Flatpak App VMs and Android Workloads.
7. Music/Media Player.
8. Clock, distributed alarms/timers, and notification entry cutover.
9. Quazar visual integration, health modal/Kiron, and release/recovery proof.
10. Shared first-release, installed-seat, provider, and corrected-forward proof
    under WL-TEST-002.

## Story execution contract

Every story below is a self-contained unit. The implementing agent must:
read the named inputs; change only the owned files; produce the named deliverable;
add the stated hostile or regression test; run the stated validation; record the
revision, command, result, and evidence path; and mark the story complete only
when the Done when condition is true. A passing compile without the named
behavioral evidence is not completion.

### WL-TEST-002 - Post-first-development-release testing and proofing

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: implementation and farm gates are complete for many slices, but first-development-release installation, operator providers, physical-seat behavior, direct-DRM rendering, guest/device integrations, and corrected-forward recovery were intentionally deferred until a signed release and its deployment inputs exist.
- Required outcome: after the first development release, execute and archive every deferred testing, proofing, and validation obligation in this epic; reconcile failures as implementation work or named external-input blockers, without weakening proof requirements.
- Current state: the queue is intentionally post-release and non-blocking for pre-release coding. No live provider, installed-seat, hardware, guest, or release-deployment claim is made.
- Dependencies: first development release, signed package/image artifacts, operator-approved provider credentials, and no more than two physical test seats.
- Deliverable: release identity and artifact admission, installed-seat baseline, provider readiness, live behavior captures, direct-DRM/GUI proof, guest/device proof, recovery/corrected-forward results, and farm commands/results with redacted operator evidence.
- Validation: run all focused farm gates first, then execute the named one-node/two-seat live checks on the exact installed release; attach evidence to the owning source epic and cross-reference this epic.
- Acceptance: tested bytes match the signed release; no fake connected/healthy state; provider absence and failure remain visible; recovery is corrected-forward; every result identifies hardware/provider authorization and stays within the two-seat cap.
- Owner: Platform collaboration and release verification.

#### WL-TEST-002 test queue

1. **Release and package admission:** build/sign the first development release; verify package/image manifests, provenance, dependency closure, upgrade/install identity, and corrected-forward payload binding.
2. **Installed baseline:** install the exact release on the governed one-node/two-seat test set; record service, Workers, Bus, storage, network, display, audio, and restart/rejoin observations.
3. **Collaboration and providers:** activate the governed SIP account; test Calls connect/reconnect/mute/consent/revocation; test deferred provider-backed collaboration and transfer behavior without asserting a fake live state.
4. **GUI and direct-DRM proof:** capture deferred shell, taskbar, style/font, Kiron, Maps, Workers, Editor, Music, and narrow/largest-text states on the real display path; include human visual review where required.
5. **Media and physical providers:** test mpv/audio/video, cache/network loss, renderer recovery, cast/handoff/DLNA, installed package/CPU behavior, and external catalog/server paths with authorized provider inputs.
6. **Guest and device integrations:** test App VM/VDI, Browser/Android/Cuttlefish, GPU/audio/input, nested-KVM, remote-session, and guest reconnect/upgrade paths when their signed images and runtime artifacts are admitted.
7. **Recovery and resilience:** run deferred one-node service/process, display/session, lock/sleep, storage/network, generation, reboot, restart, recovery, and corrected-forward drills; record failures rather than converting unavailable hardware into passes.
8. **Reconciliation:** map every result to its owning epic, archive release evidence, reopen implementation work for regressions, and retain named operator blockers for missing artifacts or authorization.

   **Moved implementation-closed UX proof queues:** `WL-UX-011` Surface
   Pro 5/6, provider, seat, and physical acceptance; and `WL-UX-014` KIRON
   renderer/live proof are post-development-release obligations owned here.
   Their implementation evidence is archived in
   `docs/worklist-archive/2026-08-15-wl-ux-011-ux-014-disposition.md`.

## Core Architecture


### WL-FUNC-011 - Build the native Mesh Collaboration Suite and hard-cut legacy collaboration

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Collaboration is split across legacy Chat, Teams rails, text-only clipboard, duplicate Files/transfers, incomplete Calls media, and an App-VM office path.
- Required outcome: one egui-native Collaboration surface has exactly Alerts, Chat, Calls, Files, Editor, and Clipboard; durable signed transport, real media, native
  office editing, and one executor replace all retired paths.
- Current state: signed envelopes, projections, native Editor foundation, POSIX/CAS Files transfer, typed cross-node executor registry, canonical AlertInbox projection, and the bounded legacy migration importer are implemented; native office editing is explicitly deferred by operator decision (2026-08-15), while the governed SIP call adapter still requires an installed account for activation and the final legacy hard cut remains.
- **Transfer executor checkpoints (2026-08-09):** only Local/Copy is admitted; Clipboard names its missing profile/Files/session/generation authority and refuses early.
  `.50` passed 2/2 plus 1/1: `docs/platform/evidence/WL-FUNC-011-2026-08-09-transfer-executor-r7.md`, `docs/platform/evidence/WL-FUNC-011-2026-08-09-transfer-executor-r8.md`.
- **Hard-cut/atomicity checkpoints (2026-08-09):** retired collaboration routes fail closed, and failed SQLite projection preserves clocks/state; BigBoy and `.50` passed:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-legacy-route-admission-r2.md`, `docs/platform/evidence/WL-FUNC-011-2026-08-09-collab-projection-atomicity-r3.md`.
- **Live-event lane identity checkpoint (2026-08-09):** signed envelopes merge only on an exact space/actor Bus lane; mismatches fail closed. Machine 193 passed 1/1:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-live-event-lane-identity-r4.md`.
- **Native-office admission checkpoint (2026-08-09):** office containers no longer fall through to lossy text editing; unsafe paths and the absent non-VCL adapter fail
  closed without opening or changing bytes. BigBoy passed 5/5: `docs/platform/evidence/WL-FUNC-011-2026-08-09-native-office-admission-r5.md`.
- **Calls provider lifecycle checkpoint (2026-08-09):** media effects refuse without a compatible provider; cleanup stays available and readiness is re-probed.
  Machine 9 passed 4/4; the SIP adapter remains fail-closed until a governed account is installed, while WebRTC/LiveKit adapters remain absent:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-calls-provider-lifecycle-r6.md`.
- **Outbound SIP ingress (2026-08-14):** bounded signed-command dial targets
  reject empty/control/oversized values before provider effects; BigBoy
  `mackesd` passed 1/1 and `mde-voice-hud` SIP passed 37/37:
  `evidence/WL-FUNC-011-2026-08-14-outbound-sip-ingress-r1.md`.
- **Native collaboration full gate (2026-08-14):** BigBoy passed 136/136: `evidence/WL-FUNC-011-2026-08-14-collab-egui-full-farm-gate-r1.md`.
- Remaining work:
- **Native office editing disposition (2026-08-15):** operator selected defer/close for the LibreOfficeKit requirement. The existing office admission boundary remains fail-closed and no VCL/GTK or `soffice` fallback is admitted. Reopen only when an approved sandboxed LibreOfficeKit runtime/package is supplied.
- **Cross-node executor registry and migration audit (2026-08-15):** the production V2 worker admits the typed Mesh/Rsync/Sftp/Http/Scrape/Multipart/Recurring/Clipboard families through the shared registry, and `mde-collab-core` provides the bounded idempotent legacy importer plus canonical `AlertInbox` projection. These were previously described as absent; no new implementation is required for this slice.
- **Calls test disposition (2026-08-15):** live SIP activation and provider proof move to `WL-TEST-002` after the first development release; FUNC-011 retains the bounded adapter and fail-closed lifecycle controls.
- **CAS read-only replay:** canonical bytes are sealed and substitution fails closed; `.196` 1/1: `evidence/WL-FUNC-011-2026-08-11-cas-readonly-replay-r377.md`.
- **CAS purge inode:** concurrent replacements cannot redirect destructive purge; `.50` 1/1: `evidence/WL-FUNC-011-2026-08-11-cas-purge-inode-binding-r428.md`.
- **Import-map inode:** hard-link aliases cannot mutate migration replay authority; BigBoy 1/1:
  `evidence/WL-FUNC-011-2026-08-11-import-map-inode-r449.md`.
- **Actor-log authenticity:** unsigned/invalid/future-schema envelopes fail before durable admission; `.196` 1/1: `evidence/WL-FUNC-011-2026-08-11-actor-log-authenticity-r375.md`.
- **Pipeline signer verification:** actor substitution cannot escape authoring; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-pipeline-signer-verification-r413.md`.
- **Descriptor source generation:** post-hash replacement fails closed; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-descriptor-source-generation-r416.md`.
- **Files CAS registration (2026-08-11):** authenticated staging, worker admission, projection, and rollback passed 15/15 on BigBoy:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-cas-stream-staging-r275.md`.
- **Calls proof attribution (2026-08-11):** incompatible adapters and
  altered/vacuous requirements fail before provider evidence; `.90` passed the
  exact regression 1/1. The remaining gap is the real external boundary—no
  governed SIP account/live call evidence is available yet, and WebRTC/LiveKit
  adapters are not implemented—not farm capacity or a multi-seat requirement:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-call-media-proof-attribution-r261.md`.
- **Calls readiness restart:** missing/corrupt readiness revokes stale media proof; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-calls-readiness-restart-r297.md`.
- **Actor-log path identity (2026-08-11):** misplaced `(space, actor)` events fail before append and after restart; `.50` passed 1/1:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-actor-log-path-identity-r311.md`.
- **Conflicting duplicate checkpoint (2026-08-10/11):** event-ID conflicts fail closed in merge, batch, and the restart-safe actor log; exact replay remains idempotent.
  `.90` passed both exact gates:
  `docs/platform/evidence/WL-FUNC-011-2026-08-10-conflicting-event-duplicates-r157.md`.
- **Alert action ID admission (2026-08-10):** unsafe IDs are rejected before
  lookup/signing; `.90` passed with `.50` format proof:
  `docs/platform/evidence/WL-FUNC-011-2026-08-10-alert-action-id-admission-r210.md`.
- **Files name-operation checkpoint (2026-08-10):** New Folder and Rename are reachable through the existing `FileOps` authority, with bounded validation and atomic
  no-replace rename; machines 193/9 passed seven focused tests: `docs/platform/evidence/WL-FUNC-011-2026-08-10-files-name-operations-r23.md`.
- **Alert delivery restart checkpoint (2026-08-09):** successful Bus/fallback
  delivery now persists bounded no-follow receipts across daemon restart;
  failed delivery retries, and traversal/forged-symlink IDs fail safely.
  Machine 196 passed the exact hostile boundary:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-alert-delivery-restart-r18.md`.
- **Federation Bus recovery checkpoint (2026-08-09):** enforcement now retries
  late Bus availability without daemon restart and folds valid trust actions
  queued during startup exactly once. Machine 194 passed three exact tests:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-federation-bus-recovery-r15.md`.
- **Collaboration Bus recovery checkpoint (2026-08-09):** activation now
  retries Bus open and atomically primes every transient lane while preserving
  durable event/log replay; one forward command projects exactly once. Machine
  193 passed three exact recovery/backfill tests:
  `docs/platform/evidence/WL-FUNC-011-WL-ARCH-009-2026-08-09-collab-bus-recovery-r22.md`.
- **Chat Bus recovery checkpoint (2026-08-09):** six mutable lanes now
  tail-prime atomically while signed messages and alert history replay without
  duplicate toasts; one forward send executes once after late storage. Machine
  9 passed four exact activation/recovery tests:
  `docs/platform/evidence/WL-FUNC-011-WL-ARCH-009-2026-08-09-chat-bus-recovery-r26.md`.
- **Notification recovery checkpoint (2026-08-09):** replacement activation skips retained Cloud alerts, preserves failed folds, and primes lanes idempotently:
  `docs/platform/evidence/WL-ARCH-009-WL-FUNC-011-2026-08-09-notify-bus-replacement-r85.md`.
- **Transfer duplicate-admission checkpoint (2026-08-10):** the daemon refuses a replayed
  legacy transfer ID instead of replacing an already-running ledger row; `.90` passed the
  hostile replacement regression: `docs/platform/evidence/WL-FUNC-011-2026-08-10-transfer-duplicate-admission-r186.md`.
- **V2 transfer no-replace checkpoint (2026-08-10):** new typed transfer admission now
  commits with an atomic same-directory no-replace install, closing the check-then-replace
  race that could let a concurrent replay overwrite the first durable row. `.90` passed the
  exact regression: `docs/platform/evidence/WL-FUNC-011-2026-08-10-v2-transfer-no-replace-r195.md`.
- **Destination-generation acknowledgement (2026-08-11):** byte-only commits fail; exact advanced generations and lost-ack replay pass. `.90` passed 1/1:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-destination-generation-ack-r259.md`.
- **V2 staging restart:** stale residue cannot block bounded create-new retry; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-v2-staging-restart-r306.md`.
  1. S1 Reconcile parity and contracts.
     - Objective: map every legacy command, route, state writer, package, and workflow to one of six sections or retirement.
     - Inputs: current parity ledger, collab types/core, archived IDs.
     - Deliverable: versioned bounded contracts for alerts, chat, calls, files, clipboard, editor, office, and transfer.
     - Depends on: none.
     - Acceptance: no conflicting Teams/channel/task/Discord/AI/App-VM office decision remains active.
     - Validation: schema/property/signature cargo tests on .90.
     - Done when: parity map and contract evidence exist.
  2. S2 Ship the six-section shell.
     - Objective: replace Communications and nested rails with one responsive Collaboration surface and contextual settings.
     - Inputs: S1 and UX-009.
     - Deliverable: six-section render/model with stable context header and route tests.
     - Depends on: S1.
     - Acceptance: Transfers is Files content; no legacy route or duplicate notification surface is reachable.
     - Validation: shell render/navigation cargo tests.
     - Done when: Dark/Light/narrow/largest-text captures pass.
  3. S3 Complete signed Chat and Alerts.
     - Objective: deliver durable direct/group chat, threads, attachments, offline replay, local find, delivery state, and canonical alert projection.
     - Inputs: collab event core and signed envelopes.
     - Deliverable: daemon authority, migration importer, and redacted alert/chat fixtures.
     - Depends on: S1, S2.
     - Acceptance: replay, attribution, ordering, and deduplication remain deterministic offline.
     - Validation: collab-core hostile and migration tests on .50.
     - Done when: legacy Chat worker/state/renderer is unreachable.
  4. S4 Complete Calls media and control.
     - Objective: provide direct/group media, provider-neutral SIP gateways, screen share, consented control, reconnect, and mute policy.
     - Inputs: media registry, SIP/RTP pieces, signed control grants.
     - Deliverable: real provider adapters and session lifecycle tests.
     - Depends on: S1, S2.
     - Acceptance: no empty provider or fake connected state; consent and revocation are auditable.
     - Validation: media/provider cargo tests and live call fixture.
     - Done when: provider availability and failure are visible.
  5. S5 Complete Files and transfer execution.
     - Objective: make Files the browser and CAS-backed transfer hub with seven typed executors, safe conflict policy, progress, cancel, retry, and cross-node
       acknowledgement.
     - Inputs: FUNC-016 and existing TransferJobV2/CAS.
     - Deliverable: executor registry, destination-generation commit, and end-to-end transfer evidence.
     - Depends on: S1, S2, FUNC-016 S1-S3.
     - Acceptance: bytes are immutable, destination commits are authenticated, and partial failure is honest.
     - Validation: transfer/CAS cargo tests on BigBoy and cross-node fixture.
     - Done when: all executor rows pass or are explicitly blocked by a named provider.
  6. S6 Complete native Editor and office sessions.
     - Objective: ship Text/Code, Document, Spreadsheet, and Presentation with sandboxed LibreOfficeKit, no VCL/App VM/compositor.
     - Inputs: existing Editor, office session contract, package policy.
     - Deliverable: native session adapter, autosave/recovery, and format fixtures.
     - Depends on: S1, S2.
     - Acceptance: open/edit/save/recover works with bounded files and no host process escape.
     - Validation: editor/office cargo and package tests on BigBoy.
     - Done when: all four kinds have render and persistence evidence.
  7. S7 Integrate Clipboard and hard-cut legacy products.
     - Objective: mount FUNC-016 rich clipboard, migrate useful data, and delete retired Chat/Teams/Files/Editor/Notification paths.
     - Inputs: S2-S6, FUNC-016.
     - Deliverable: migration report, negative scans, and one release surface.
     - Depends on: S3-S6.
     - Acceptance: six sections are the only primary collaboration navigation and no duplicate authority remains.
     - Validation: architecture, secret, supersession, package, and shell gates.
     - Done when: fresh checkout has no retired production route/worker/package.
  8. S8 Prove collaboration release.
     - Objective: run offline/online, permission, media, transfer, editor, clipboard, migration, recovery, and at-most-two-seat live acceptance.
     - Inputs: S1-S7, CRIT-006.
     - Deliverable: signed evidence bundle and visual captures.
     - Depends on: S7.
     - Acceptance: missing external media or hardware is recorded as a blocker, never a pass.
     - Validation: farm workspace gates and named live-seat proofs.
     - Done when: release evidence lists every section and failure path.
- Scope: Owns the one Collaboration surface, signed contracts, Chat, Calls, Files executors, Editor/office, Alerts, migration, packaging, and hard cut. Clipboard
  transport details belong to FUNC-016.
- Relevant files/components: mde-collab-types, mde-collab-core, mackesd collaboration/files workers, mde-shell-egui collaboration/editor, LibreOfficeKit packaging, and
  transfer/CAS modules.
- Dependencies: FUNC-016, ARCH-009, ARCH-010, UX-009, CRIT-006, and CRIT-007.
- Acceptance criteria:
  1. Exactly six primary sections exist and all retired collaboration surfaces are unreachable.
  2. Signed offline replay, real media, CAS transfer, native office, and rich clipboard pass focused hostile tests.
  3. At-most-two-seat release proof records real providers, partial failures, and corrected-forward recovery.
- Verification method: collab/file/editor/media cargo suites, architecture/secret/package gates, visual captures, and live provider tests; route long jobs to BigBoy.
- Origin or merged source IDs: NOTIFY-CHAT, EDITOR-*, FILEMGR-*, TEAMS-*, CLIPBOARD-*, VOICE-*; 2026-08-03 Mesh Collaboration survey.

### WL-FUNC-021 - Deliver the Spotify-class Music workspace and service parity
- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Music has a direct Airsonic panel and incomplete daemon authority, media playback, library/Jellyfin, offline cache, discovery, casting, handoff, and live proof.
- Required outcome: daemon-owned typed music catalog/queue/playback/cache; real mpv audio/video; local/Jellyfin, discovery, cast, handoff, and live proof.
- Current state: release 11/daemon authority run on five seats; Dell/CPU/NWS/provider-loss pass; Bus fold:
  `evidence/WL-FUNC-021-WL-ARCH-009-2026-08-09-media-server-bus-transaction-recovery-r82.md`; renderer/cast/handoff implementation remains. Live-seat, provider, package, and continuity proof is post-release validation owned by `WL-TEST-002`.
- **Cast adapter, discovery, command dispatch, ownership, and typed handoff (2026-08-15):** `mde-musicd` now owns a bounded numeric-address Cast target, bounded DIAL identity parser, CASTV2 TLS/protobuf seam, blocking default-media-receiver load/play/pause/seek dispatch, generation-bound atomic ownership records, and a typed Cast handoff request using `rust_cast`; the durable peer handoff records now carry backward-compatible target kind/identity fields defaulting to `mesh_seat`. Focused Cast tests pass 12/12 and handoff compatibility tests pass 15/15. The reachable Xiaomi MIBOX4 proof is recorded in `evidence/WL-FUNC-021-2026-08-15-cast-target-availability-r1.md`; provider integration into the mesh handoff commit remains.
- **Projection validation:** bad snapshots retain last-good; zero is refused; UI 4/4 `.50`, daemon 1/1 `.90`: `evidence/WL-FUNC-021-2026-08-06-projection-validation-r2.md`.
- **Media hardening (2026-08-06):** media-core 250/250 on BigBoy; four bounded Music proof-helper self-tests pass.
  Live renderer/provider acceptance is owned by `WL-TEST-002`; no second-seat proof is required. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-media-hardening-r2.md`.
- **Provider consistency (2026-08-09):** restart selection and stale fallback invalidation passed `.90`; evidence: `evidence/WL-FUNC-021-2026-08-09-provider-restart-binding-r4.md`.
- **Music Bus replacement (2026-08-10):** `.90` passed: `docs/platform/evidence/WL-FUNC-021-2026-08-10-music-bus-reopen-r158.md`.
- **Bounded media config (2026-08-11):** shared-folder JSON caps at 64 KiB and rejects symlinks; BigBoy: `evidence/WL-FUNC-021-2026-08-11-media-config-bound-r226.md`.
- **Navidrome command timeout (2026-08-11):** systemctl/setup calls fail closed at the shared deadline; BigBoy: `evidence/WL-FUNC-021-2026-08-11-navidrome-command-timeout-r226.md`.
- **Bounded service registration hostname (2026-08-11):** `/etc/hostname` caps at 255 bytes; BigBoy passed 1/1: `evidence/WL-FUNC-021-2026-08-11-service-hostname-bound-r230.md`.
- **Bounded Navidrome commands (2026-08-11):** systemctl uses shared 15s boundary; BigBoy passed 3/3: `evidence/WL-FUNC-021-2026-08-11-navidrome-command-bound-r231.md`.
- **Navidrome setup bound (2026-08-11):** re-provision shares 15s timeout; `.90` 3/3: `evidence/WL-FUNC-021-2026-08-11-navidrome-setup-timeout-r232.md`.
- **Artwork byte bound (2026-08-11):** non-regular/over-4M reads and
  oversized writes refuse; `.50` passed the focused gate. This slice is
  complete; remaining Music work is renderer/provider/cast/handoff/live proof:
  `evidence/WL-FUNC-021-2026-08-11-artwork-byte-bound-r222.md`.
- **Revoked renderer generation:** device loss blocks in-flight audio/queue commit; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-revoked-renderer-generation-r429.md`.
- **Transcode generation binding:** source/session substitution fails closed; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-transcode-generation-binding-r432.md`.
- **Queue persistence rollback:** failed durable writes roll memory back and report failure; BigBoy 1/1: `evidence/WL-FUNC-021-2026-08-11-queue-persistence-rollback-r410.md`.
- **Media-source heartbeat:** impossible retained observations cannot restore reachability; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-media-source-heartbeat-r313.md`.
- **Provider identity:** endpoint equivocation revokes fallback; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-provider-identity-equivocation-r314.md`.
- **Media UI full farm gate (2026-08-14):** `.90` passed 114/114; H.264 remains transcode-only under the mpv baseline: `evidence/WL-FUNC-021-2026-08-14-media-full-farm-gate-r1.md`.
- **Manifest/Jellyfin identities:** forged/duplicate manifest items and server IDs fail closed; `.196`/`.50` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-media-manifest-item-identity-r317.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-server-identity-r316.md`.
- **Jellyfin cache/sync:** exact digests and URL segments reject content/path substitution; `.50`/`.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-jellyfin-cache-digest-r319.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-sync-path-identity-r320.md`.
- **Jellyfin metadata generation:** stale/equivocal replay cannot roll back cache; BigBoy 1/1: `evidence/WL-FUNC-021-2026-08-11-jellyfin-metadata-generation-r411.md`.
- **Playlist persistence:** symlink-safe atomic saves/loads; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-playlist-symlink-atomicity-r318.md`.
- **Finite resume:** nonfinite samples preserve valid durable state; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-finite-resume-state-r321.md`.
- **Roaming lease:** playback arms only after exact durable ownership; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-roaming-lease-publication-r322.md`.
- **Party election:** same-sequence deterministic winners replace losing authority; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-party-election-key-r324.md`.
- **Jellyfin transcode authority:** hostile redirects cannot escape server/item identity; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-jellyfin-transcode-authority-r323.md`.
- **Proxy commitment/route authority:** post-commit failure and in-flight Bus replacement fail closed; `.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-airsonic-response-commit-r325.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-inflight-route-r326.md`.
- **Music cache/retry:** exact path identity and saturating backoff survive hostile inputs; `.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-music-cache-path-identity-r327.md`, `evidence/WL-FUNC-021-2026-08-11-reconnect-backoff-overflow-r328.md`.
- **Cast/stream identities:** renderer equivocation and ambiguous network authorities fail closed; `.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-cast-discovery-equivocation-r329.md`, `evidence/WL-FUNC-021-2026-08-11-stream-authority-admission-r330.md`.
- **Jellyfin request paths:** remote IDs cannot escape endpoint segments; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-jellyfin-client-path-identity-r331.md`.
- **Player/control generations:** replacement signals and malformed numeric controls fail closed; BigBoy 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-player-replacement-generation-r332.md`, `evidence/WL-FUNC-021-2026-08-11-finite-media-controls-r333.md`.
- **Library/browse identities:** playback paths and series trees reject substituted durable/provider state; BigBoy/`.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-library-playback-path-identity-r334.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-browse-series-identity-r335.md`.
- **Frame layout authority:** malformed RGBA geometry/length cannot prove playback; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-frame-layout-authority-r336.md`.
- **Production Navidrome:** failed store repair stays withdrawn; BigBoy 1/1: `evidence/WL-FUNC-021-2026-08-11-production-navidrome-withdrawal-r337.md`.
- **Subtitle/redirect authority:** ambiguous subtitle sources and Jellyfin cross-authority redirects fail closed; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-subtitle-source-admission-r338.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-redirect-authority-r339.md`.
- **Transport redirect authority:** caller policy cannot forward credentials to a provider-selected host; `.90` 1/1:
  `evidence/WL-FUNC-021-2026-08-11-jellyfin-transport-redirect-r442.md`.
- **Lockscreen media identity:** replacement playback cannot inherit retained controls; BigBoy exact:
  `evidence/WL-FUNC-021-2026-08-11-lockscreen-media-identity-r455.md`.
- **Video/audio generations:** replacement frame adjustments and sink choice explicitly reset; `.170`/`.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-video-adjustment-revocation-r340.md`, `evidence/WL-FUNC-021-2026-08-11-audio-sink-revocation-r342.md`.
- **Provider item equivocation:** OpenSubtitles and Jellyfin conflicting identities fail closed; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-opensubtitles-equivocation-r341.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-item-admission-r343.md`.
- **Capture path authority:** device aliases/traversal fail closed; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-capture-device-path-authority-r344.md`.
- **Codec/yt-dlp authority:** baseline capabilities and extracted URLs fail closed without optional/runtime authority; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-universal-codec-capability-r345.md`, `evidence/WL-FUNC-021-2026-08-11-ytdlp-authority-boundary-r346.md`.
- **Smoke proof:** success requires Playing, audio, and nonblank frame; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-media-smoke-proof-integrity-r347.md`.
- **Media roster authority:** publication watermark and malformed-state revocation prevent stale gateways; `.196`/`.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-media-roster-watermark-r348.md`, `evidence/WL-FUNC-021-2026-08-11-media-roster-revocation-r349.md`.
- **Media menu identity:** stale track actions cannot target replacement media; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-media-menu-track-identity-r350.md`.
- **Music server generation:** old-server playback stops before replacement authority; `.90` 1/1: `evidence/WL-FUNC-021-2026-08-11-music-server-generation-revocation-r351.md`.
- **Music source equivocation:** provider order cannot invent reachability/capabilities; `.90` 1/1: `evidence/WL-FUNC-021-2026-08-11-music-source-equivocation-r352.md`.
- **Music catalog/radio identity:** conflicting current rows and withdrawn details cannot publish stale UI intent; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-music-current-catalog-equivocation-r354.md`, `evidence/WL-FUNC-021-2026-08-11-music-radio-detail-revalidation-r353.md`.
- **Airsonic/queue identity:** substituted songs and conflicting queue IDs fail closed; `.90`/`.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-airsonic-track-identity-r355.md`, `evidence/WL-FUNC-021-2026-08-11-queue-entry-identity-r356.md`.
- **Queue/mpv generations:** framed CAS and ordered current-load events reject substituted durable/frame state; `.90` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-queue-cas-framing-r357.md`, `evidence/WL-FUNC-021-2026-08-11-mpv-frame-generation-r358.md`.
- **Credential/state files:** no-follow bounded regular-file reads reject substitution; `.90` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-credential-file-identity-r359.md`, `evidence/WL-FUNC-021-2026-08-11-state-file-nofollow-r362.md`.
- **Daemon/seat identity:** kernel singleton and PipeWire serial/process binding revoke duplicate/recycled owners; `.90` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-daemon-kernel-singleton-r360.md`, `evidence/WL-FUNC-021-2026-08-11-seat-audio-object-identity-r361.md`.
- **MPRIS generation:** seek binds the audible track, not an advanced queue cursor; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-mpris-audible-generation-r363.md`.
- **Workspace request binding:** durable IDs bind exact digests; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-workspace-request-binding-r244.md`.
- **Action-ledger privacy:** restart enforces the six-hour epoch and rejects future rows; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-action-ledger-privacy-r309.md`.
- **Live-ledger saturation:** full live epochs reject new mutations without eviction; `.90` 1/1: `evidence/WL-FUNC-021-2026-08-11-live-ledger-saturation-r364.md`.
- **Audible fallback/loss:** inaudible candidates cannot suppress fallback; audible loss drains its valid tail and preserves handoff; focused farms 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-audible-fallback-authority-r366.md`, `evidence/WL-FUNC-021-2026-08-11-audible-provider-loss-tail-r385.md`.
- **Bounded bookmark link probe (2026-08-11):** HTTP curl hangs fail closed; `.50` passed 1/1: `evidence/WL-FUNC-021-WL-ARCH-009-2026-08-11-bookmark-probe-timeout-r235.md`.
- **PipeWire dump bound (2026-08-11):** `pw-dump` output capped at 16 MiB before JSON parsing; `.50` passed: `evidence/WL-FUNC-021-2026-08-11-pw-dump-bound-r223.md`.
- **Cast URL admission:** unsafe/local/credential-bearing URLs refused; BigBoy: `evidence/WL-FUNC-021-2026-08-10-cast-media-url-admission-r184.md`.
- **Full Music daemon farm gate (2026-08-14):** `mde-musicd` passed 274/274 tests after eliminating the parallel `HOME` mutation race in cover-art proof:
  `evidence/WL-FUNC-021-2026-08-14-music-full-farm-gate-r1.md`.
- **Direct URL admission:** malformed/credential-bearing/unsafe URLs refused; `.90`: `evidence/WL-FUNC-021-2026-08-10-direct-media-url-admission-r214.md`.
- **Named-detail/activation/NWS release-11 checkpoint (2026-08-08):** identity-bound details, one daemon/shell per seat, Dell records, and five-seat recovery pass:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-seat-activation-release10-r1.md`; `docs/platform/evidence/WL-FUNC-021-2026-08-08-nws-recovery-release11-r1.md`.
- **Signed live-radio:** release 8 Dell/15 C-SPAN capture passed; remaining seats/judgment open: `evidence/WL-FUNC-021-2026-08-08-live-radio-release8-r1.md`.
- **Library checkpoint (2026-08-06):** typed collections replace Airsonic rows; UI 44/44 on `.50`, fmt `.90`; `evidence/WL-FUNC-021-2026-08-06-daemon-library-r1.md`.
- **Search checkpoint (2026-08-06):** retained typed search renders; provider search is fallback; UI 45/45 `.50`; `evidence/WL-FUNC-021-2026-08-06-daemon-search-r1.md`.
- **Drain guards (2026-08-06):** search replay and duplicate Jellyfin identities pass `.90`; live-seat RPM ownership self-test and read-only probe pass.
- **Cache checkpoints:** truncation refused; replacement keeps last-good; live/package proof open; evidence: `evidence/WL-FUNC-021-2026-08-09-cache-index-atomic-r8.md`.
- **Music cache completeness (2026-08-10):** `.90` passed truncated/replaced-file refusal: `docs/platform/evidence/WL-FUNC-021-2026-08-10-cache-completeness-r208.md`.
- **mpv/recovery checkpoints:** retry/resume passed 239/239; real nonblank playback plus playlist/replacement continuation passed 3/3 on BigBoy.
  Live proof remains: `evidence/WL-FUNC-021-2026-08-06-media-recovery-r1.md`, `evidence/WL-FUNC-021-2026-08-09-mpv-playlist-continuation-r11.md`.
- **Daemon Album/download/workerless:** retained albums emit typed play; bounded actions pass daemon 168/168 and UI 47/47; construction starts no worker.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-managed-download-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-embedded-workerless-r1.md`.
- **Typed target handoff checkpoint (2026-08-06):** fresh idle peers publish typed `transfer`; stale/owning peers remain browse-only. `.50` passed 48/48; live proof remains:
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-target-handoff-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-peer-targets-r1.md`.
- **Handoff routing:** owner/bystander pumps preserve another seat's completion; BigBoy 12/12; evidence: `evidence/WL-FUNC-021-08-09-handoff-target-routing-r95.md`.
- **Cast:** bounds, live CASTV2 connection, DIAL identity, media load, pause, and generation-bound ownership passed; mesh handoff integration remains: `evidence/WL-FUNC-021-2026-08-06-cast-bounds-r1.md`, `evidence/WL-FUNC-021-2026-08-09-chromecast-async-discovery-r12.md`, `evidence/WL-FUNC-021-2026-08-15-cast-adapter-foundation-r1.md`.
- **Live provider loss:** seat 15 recovered with zero restarts; audible continuity remains: `evidence/WL-FUNC-021-2026-08-08-live-provider-loss-release11-r1.md`.
- **Provider-loss reconnect:** bounded `timeOffset` resume clears buffered-ahead samples, preserves cache, and refuses arbitrary URLs; focused gates pass.
  Seat 15 recovers the provider while daemon/cached projections remain available; audible in-progress continuity remains open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-network-loss-reconnect-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-reconnect-timeout-r1.md`.
- **Zero-audio failover:** empty streams cannot suppress fallback; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-zero-audio-provider-failover-r289.md`.
- **Cast loopback:** bounded discovery/control/seek passes; live CastV2 channel and command-dispatch seam are implemented; media/ownership live proof remains post-release validation in `WL-TEST-002`: `evidence/WL-FUNC-021-2026-08-06-cast-loopback-r1.md`.
- **Two-seat handoff:** exact-once transfer, mismatch/stale refusal, and atomic records pass `.50`/`.90`/`.170`; live boundary:
  `evidence/WL-FUNC-021-2026-08-08-two-seat-owner-handoff-r1.md`, `evidence/WL-FUNC-021-2026-08-09-handoff-atomic-r9.md`.
- **Cast runtime audit:** the authorized Xiaomi MIBOX4 Cast target is now reachable and the Rust adapter has live CASTV2/media proof. Mesh-owner receiver integration remains open. Any installed-seat or continuity capture is post-release validation coordinated by `WL-TEST-002`, with no two-seat proof requirement for this epic.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-runtime-audit-r1.md`.
- **Cast-admission checkpoint (2026-08-06):** URLs, titles, and HTTP endpoints reject oversized/control-bearing input before the network gate; BigBoy tests
  passed 20/20. Live Cast adapter/media proof is recorded separately; mesh-owner receiver integration and installed-seat capture remain post-release validation owned by `WL-TEST-002`.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-admission-r1.md`.
- **Two-catalog outage checkpoint (2026-08-06):** source projection retains two admitted variants under one logical queue track.
  Failed-first/healthy-second decoding and BigBoy gates pass; live outage, mid-track resume, and hardware/package proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-two-catalog-outage-r1.md`.
- **Jellyfin outage:** known-good cache survives failures; truncated manifests refused; live proof remains: `evidence/WL-FUNC-021-2026-08-06-jellyfin-outage-r1.md`.
- **GUI authority:** both Music surfaces consume daemon projections, start no provider/playback worker, and require an authenticated writer; `.50` passed:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-standalone-daemon-authority-r1.md`.
- **Renderer recovery:** failure revokes playback/MPRIS; reacquisition resumes the exact finite track unless cancelled; live audible/two-seat proof remains:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-renderer-recovery-r1.md`.
- **Real-mpv UI (2026-08-07):** frames clear; 110/110 UI and 257 tests passed; physical proof remains: `docs/platform/evidence/WL-FUNC-021-2026-08-07-media-render-clear-r1.md`.
- **Continuation:** daemon 182/182, roaming 18/18, reconnect 8/8, router 26/26, and Dell CPU passed:
  `evidence/WL-FUNC-021-2026-08-06-roaming-root-loss-r1.md`, `evidence/WL-FUNC-021-2026-08-06-reconnect-loop-audit-r1.md`.
- **Live boundary:** same-provider resume and package/gateway gates pass; the authorized Cast target is now available and media control is proven.
  Live loss, renderer recovery, mesh handoff, auth/rotation, and two-seat CPU/NWS remain open; no third seat is required.
  `evidence/WL-FUNC-021-2026-08-07-provider-loss-audit-r1.md`, `evidence/WL-FUNC-021-2026-08-07-cast-runtime-audit-r2.md`.
- **Seat-15 CPU:** bounded samples found Syncthing convergence; load stayed below capacity and no daemon was pegged; steady-state retest remains:
  `evidence/WL-FUNC-021-2026-08-10-seat15-cpu-attribution-r147.md`, `evidence/WL-FUNC-021-2026-08-10-seat15-cpu-retest-r157.md`.
- **Idle-state coalescing:** transition/heartbeat-preserving suppression passes BigBoy: `evidence/WL-FUNC-021-2026-08-10-idle-state-coalescing-r155.md`.
- **State revision replay:** rollback/equivocation fails closed; BigBoy passed 1/1: `docs/platform/evidence/WL-FUNC-021-2026-08-11-state-revision-replay-r269.md`.
- **Live provider audio:** Airsonic track `23427` completed with 6,287,357/6,717,440 monitored samples nonzero; temporary capture was removed.
  Provider-loss resume, speaker judgment, and authenticated mutation remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-live-audio-capture-r1.md`.
- **Live Music DRM:** seat 15 produced a settled 1920x1080 direct-DRM EGL frame; the Music verifier accepted 15 separated foreground components.
  The temporary drop-in was removed, service returned active with zero restarts, and full acceptance/loss/handoff/package proof remains open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-live-drm-frame-r1.md`;
  `install-helpers/verify-music-drm-proof.py`.
- **RPM/install:** F44 release 5 passed payload/size and Dell CPU proof; seat 15 remained release 4: `evidence/WL-FUNC-021-2026-08-06-dell-release5-cpu-r1.md`.
- **Artwork/pagination:** daemon/UI/shell gates pass; release 6 is live on Dell/seat 15 with distinct bounded pages and local JPEG art.
  Open: renderer, provider-loss, cast, handoff, radio playback, and two-seat CPU/NWS; no third seat is required.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-07-music-artwork-release6-r1.md`.
- **Mutation authorization delivery:** domain-separated Ed25519 capabilities bind digest/scope/expiry/replay; daemon receives only the public key.
  Shared types 431/431 and daemon 174/174 pass; live authorized mutation and installed-seat rotation remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-delivery-audit-r2.md`.
- **Mutation authorization package:** RPM assets/systemd/helper and dependency checks declare required `openssl`/`curl` in the fresh base header.
  Installed-seat provisioning, mutation, and rotation proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-package-audit-r2.md`;
  prior audit: `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-package-audit-r1.md`.
- **Reusable live-seat gate:** self-test and bounded seat-15 read-only checks pass; the 15-second song probe left no client process and claims no audible/rendered acceptance.
- **Queue durability:** atomic replacement preserves last-good and cleans failed staging; `.50` 14/14: `evidence/WL-FUNC-021-2026-08-09-queue-atomic-persistence-r1.md`.
  1. S1 Freeze catalog/provider authority.
     - Objective: make mde-musicd the only catalog/source/queue authority for Subsonic, local, Jellyfin, and approved providers.
     - Inputs: music types/domain, resource catalog, Jellyfin store.
     - Deliverable: bounded source contracts, provider selection, credentials redaction, and hostile tests.
     - Depends on: FUNC-019 S1/S2.
     - Acceptance: UI cannot invent a server, source, track, or queue state.
     - Validation: mde-musicd and Jellyfin cargo tests on .50.
     - Done when: source snapshots and provider failure evidence exist.
  2. S2 Prove real playback.
     - Objective: decode real audio/video with mpv, publish frame/audio/position/error, and recover from pause/seek/end/network loss.
     - Inputs: S1, mde-media-core, PipeWire/fixture assets.
     - Deliverable: engine adapter and nonblank-frame/resolved-audio fixtures.
     - Depends on: S1.
     - Acceptance: no fake success, silent fallback, unbounded event queue, or stale position.
     - Validation: mde-media-core --features mpv cargo tests/doctests on BigBoy.
     - Done when: frame/audio metrics and failure traces are recorded.
  3. S3 Ship workspace and daemon-owned controls.
     - Objective: render Home, Browse, Search, Queue, Now Playing, albums, artists, playlists, bookmarks, and typed transport controls.
     - Inputs: S1/S2 and UX-009/012.
     - Deliverable: Music UI model/render and signed Bus action integration.
     - Depends on: S1, S2.
     - Acceptance: GUI has no competing worker/store/playback authority.
     - Validation: mde-music-egui and shell Music cargo tests on .50.
     - Done when: responsive captures and action traces pass.
  4. S4 Complete library, Jellyfin, cache, and bookmarks.
     - Objective: load saved servers/profiles, browse/play libraries, download/cache bounded content, resume supported bookmarks, and report unavailable data honestly.
     - Inputs: S1-S3, Jellyfin store, cache policy.
     - Deliverable: library/cache/bookmark flows with atomic persistence and offline fixtures.
     - Depends on: S3.
     - Acceptance: credentials remain 0600; crash preserves old or new complete store; offline uses verified cache only.
     - Validation: music/Jellyfin/cache cargo tests and secret scan.
     - Done when: two-catalog and network-loss evidence exists.
  5. S5 Complete discovery, cast, and peer handoff.
     - Objective: discover bounded targets, perform typed DLNA/mesh handoff, seek after play, and preserve owner-yield/target-resume semantics.
     - Inputs: media cast core, peer/resource contracts.
     - Deliverable: discovery/cast/handoff adapters and refusal tests.
     - Depends on: S2-S4 and FUNC-019 S4.
     - Acceptance: malformed targets, nonfinite positions, failed seek, and conflicting owners fail closed.
     - Validation: media cast cargo tests on BigBoy and live DLNA/peer fixture.
     - Done when: handoff evidence names every unavailable target.
  6. S6 Complete release proof.
     - Objective: verify visual/audio playback, controls, cache, cast, handoff, package, RPM, Dell, and seat-15 acceptance.
     - Inputs: S1-S5 and CRIT-006.
     - Deliverable: signed Music/Media evidence bundle and rendered captures.
     - Depends on: S5.
     - Acceptance: live gaps remain explicit and do not become green by inference.
     - Validation: farm music/media suites, RPM gates, and named live-seat commands.
     - Done when: all required provider, hardware, and package results are recorded.
- Scope: Owns Music workspace/service, Media Player core/UI/Jellyfin, catalog/playback/cache/bookmarks, discovery/casting/handoff, shell integration, packaging, and
  proof. Generic Workload and collaboration transport remain elsewhere.
- Relevant files/components: mde-musicd, mde-music-egui, mde-media-core, mde-media-egui, mde-jellyfin, shell Music mount, Bus/resource contracts, PipeWire/mpv, and
  RPM/live scripts.
- Dependencies: FUNC-019, ARCH-010, UX-009, UX-012, CRIT-006/007.
- Acceptance criteria:
  1. Daemon authority and real mpv frame/audio playback pass hostile and fixture tests.
  2. Library/Jellyfin/cache/bookmark, discovery/cast, handoff, and network-loss flows are typed and bounded.
  3. At-most-two-seat visual/audio/package evidence proves the shipped release or names blockers.
- Verification method: use @farm:{cargo test -p mde-musicd}
  @farm:{cargo test -p mde-media-core --features mpv}
  @farm:{cargo test -p mde-media-egui}
  and shell/RPM/live gates with BigBoy for the longest media job.
- Origin or merged source IDs: Spotify-class Music survey; archived WL-FUNC-007 and MEDIA-1..17; 2026-08-05/06 Music and Media evidence.
## User Interface And Experience

### WL-UX-009 evidence disposition — Editor large-text menubar correction (2026-08-02)

The shared `MenuBar` correction is accepted for the Editor Light/Largest
direct-DRM slice. The constrained nested editor pane now uses an explicit
two-row layout with a single-line horizontally scrollable menu strip, keeping
`Help`, formatting controls, the document body, status row, details rail, and
taskbar visible without accidental vertical expansion. Farm evidence is
`mde-egui` 269/269 and `mde-editor-egui` 407/407; Dell `.138` ran payload
`4808bd30bfa72ab386056cd1ecbc4d6aac0251a144609aedcee3e209b8dc888c` with an
active zero-restart service. Accepted proof:
`evidence/WL-UX-009-2026-08-02-138-editor-light-largest-4808bd30.png`,
SHA-256 `b1e0bafea6d63cd88f0979d11024da56a05611115f1e8ee52bbc4c19035371cb`.
This is a closed validation slice, not closure of WL-UX-009 or a production
readiness claim; the full current-payload route/profile matrix and remaining
hardware/boundary evidence are still required.

- Proof-route follow-up (2026-08-02): the explicit Files direct-DRM proof route
  is reasserted at shell construction and frame entry, but the live readback
  still lands on the Auto/clock surface after later automatic navigation drains.
  Add one final proof-only route assertion immediately before central workspace
  rendering, preserve ordinary navigation behavior, farm-test the route seam,
  and recapture the Files/Airsonic evidence before accepting the slice. This is
  a route-harness correction only; no readiness claim follows from it.
- Farm-integrity finding (2026-08-02): the current release source cannot compile
  the existing This Node hostile-fixture test because a JSON `\\u0000` escape is
  interpreted by Rust as an invalid source escape. Convert only that fixture to
  Rust's braced Unicode escape form, rerun the release build and focused test,
  and keep all visual/readiness claims open until the exact candidate is proven.
- Farm-integrity resolution (2026-08-02): the synchronized farm source already
  contains the braced `\\u{0000}` fixture form; the stale first release sync was
  discarded. The rerun release build on BigBoy completed successfully and
  produced exact payload `b888b0d163de8369b554569d5a75f3f17f257d8581f1e2558d14a3d479435f0c`.
- Proof-route resolution (2026-08-02): after the approved temporary
  `require_login_at_boot:false` proof fixture, exact candidate `b888b0d1…` on
  `.138` rendered the explicit Files route rather than the boot curtain. Visual
  inspection accepts the shared `FILES` frame, complete ten-action NODE ACTIONS
  inventory, reachable mesh peer, and `Airsonic upload · Music-owned` action;
  PNG SHA-256 is `64be0f4af1cdf26ecfc66e96172cf76a6234b64caed7b842fe2fec9c31e3b329`
  in the DRM evidence ledger. The temporary unlock, candidate binary, and proof
  drop-in were removed; `.138` is restored to payload `20955383…`,
  `require_login_at_boot:true`, Dark/Construct/Default/Normal, active service,
  and zero recorded restarts. This closes only the Files/Airsonic proof-route
  slice; Dell adoption, the remaining matrix, strict linear scanout, and
  WL-UX-009 readiness remain open.
- Car Light palette implementation slice (2026-08-02): the Car profile currently
  installs `AutoSync3` as the whole egui scheme, so the Auto Mode dashboard cannot
  honor a persisted Light choice even though its cards use shared palette tokens.
  Preserve AutoSync3's vehicle-specific accents/skin while pass-through rendering
  the persisted Dark/Light surface palette to AutoHome. Add a focused Light-vs-Dark
  render contract, farm-test it, then recapture exact current-payload Car Light /
  Largest direct-DRM frames on both seats before accepting the open cell.
- Car Light palette resolution (2026-08-02): candidate
  `e30f36cd562f729f91620ef3842827190bbf3b055bcd8126072630d1dedcd0ee` passed
  the focused Car suite 14/14. Visually accepted native Light/Largest Auto Mode
  frames were captured on `.138` (1920x1080,
  `79355ca02ed2a8086d0f9cd14dcfa411233e1aad46f8496ab0b885f9c781332b`) and
  Dell `.225` (1366x768,
  `39361bc641021fd0ed4e5ec7c4dd92b86343d944c5ce2bab3432c2feffa4dcbb`), with
  Light surface ground/cards and AutoSync3 accents. Both seats were restored to
  payload `20955383…`, secure Dark/Construct/Default/Normal, active service,
  zero restarts, and no proof drop-in. The proof-only logical-width override
  intentionally leaves each PNG at native physical dimensions while bounding
  content to the requested 800 logical-pixel viewport. This closes the Car
  Light/Largest narrow palette cell; strict linear scanout, the remaining
  route/profile matrix, and WL-UX-009 readiness remain open.
- Dell Terminal narrow recapture (2026-08-02): validate the current release
  candidate `e30f36cd…` on Dell `.225` for Terminal Dark desktop, Dark narrow
  (`800` logical width), Light desktop, and Light/Largest narrow. Inspect the
  full `TERMINAL` identity, command/session controls, taskbar contrast, and
  bounded body; accept only visually complete frames and restore secure seat
  state afterward.
- Dell Terminal recapture resolution (2026-08-02): candidate `e30f36cd…` on
  Dell `.225` passed visual inspection for Dark desktop, Dark narrow (`800`
  logical), Light desktop, and Light/Largest narrow. The full `TERMINAL`
  identity, menu/session controls, taskbar contrast, and bounded body are
  present; the earlier `TER…` interpretation is superseded. Evidence hashes
  are recorded in the DRM ledger. Dell was restored to payload `20955383…`,
  secure Dark/Construct/Default/Normal, active service, zero restarts, and no
  proof drop-in. This closes the Dell Terminal visual slice only; the remaining
  route/profile matrix, VDI guest readiness, strict linear scanout, and
  WL-UX-009 readiness remain open.
- Dell Editor current-candidate recapture (2026-08-02): validate candidate
  `e30f36cd…` on Dell `.225` for Editor Dark desktop, Dark narrow (`800`
  logical width), Light desktop, and Light/Largest narrow. Inspect direct-entry
  sidebar collapse, the shared `EDITOR` identity, internal menu/toolbar
  reachability, document/status/details geometry, and taskbar contrast; accept
  only complete frames and restore secure seat state afterward.
- Dell Editor recapture resolution (2026-08-02): candidate `e30f36cd…` on
  Dell `.225` passed visual inspection for Dark desktop, Dark narrow (`800`
  logical), Light desktop, and Light/Largest narrow. Direct-entry sidebars are
  collapsed; the Mesh Teams editor host chrome, editor toolbar/menu, document,
  status/details geometry, and taskbar remain bounded. Evidence hashes are in
  the DRM ledger; no guest/VDI pixels are claimed. Dell was restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes the Dell Editor visual slice only;
  the remaining route/profile matrix, guest VDI readiness, strict linear
  scanout, and WL-UX-009 readiness remain open.
- Dell Files and Mesh Teams recapture (2026-08-02): validate candidate
  `e30f36cd…` on Dell `.225` for Files and Mesh Teams across Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow. Inspect
  Files' complete ten-action node inventory and sync/status lanes, plus Mesh
  Teams' shared identity strip, channel/app rails, and bounded body. Accept
  only visually complete frames and restore secure seat state after the batch.
- Mesh Teams Light contrast finding (2026-08-02): Dell `.225` current-candidate
  Mesh Teams Dark desktop and Dark narrow are readable, but Light desktop and
  Light/Largest narrow render Activity/body and rail copy with the Dark
  `TEXT`/`TEXT_DIM` values under the Light surface, producing washed-out,
  low-contrast content. Reject both Light cells; resolve Mesh Teams-owned text
  tokens through the shared runtime palette, add a Light render assertion, farm
  test, and recapture Dell before accepting the route.
- Mesh Teams Light contrast resolution (2026-08-02): candidate
  `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236` maps the
  Activity and frame-owned text tokens through the shared runtime palette;
  BigBoy `mde-collab-egui --lib` passed 130/130, including an explicit
  Light-mode render assertion for Activity and Mesh Teams rail text. Dell Light desktop and
  Light/Largest narrow frames were recaptured after the normal page crossfade
  settled and visually accepted; their hashes and links are in the DRM ledger.
  The earlier transition frames remain rejected diagnostic evidence. Dell was
  restored to payload `20955383…`, secure Dark/Construct/Default/Normal,
  active service, zero restarts, and no proof drop-in. This closes only the
  Dell Files/Mesh Teams visual slice and Mesh Teams Light contrast finding;
  VDI guest readiness, strict linear scanout, the remaining matrix, and
  WL-UX-009 readiness remain open.
- VDI guest endpoint audit (2026-08-02): a fresh read-only probe of enrolled
  validation seats `.15`, `.138`, `.145`, and Dell `.225` found no open RDP,
  VNC, SPICE/VDI, or Sunshine endpoint on the approved validation ports.
  The approved boundary remains documented, but no guest framebuffer, guest
  input, or VDI readiness claim is made; retain this as an external-state
  evidence gap rather than styling the guest surface or claiming readiness.
- Current candidate Car matrix recapture (2026-08-02): validate exact release
  `ae51c124…` on both direct-DRM seats for Car Light/Largest narrow after the
  AutoHome palette resolution. Confirm the AutoSync3 vehicle skin remains
  intact while the persisted Light surface palette is honored; accept only
  complete, readable cockpit frames and restore secure seat state afterward.
- Current candidate Car matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Car Light/Largest narrow on `.138` and Dell `.225`.
  Both direct-DRM frames show the complete Auto Mode cockpit, Light surface
  palette, preserved AutoSync3 accents, and bounded large-text cards; hashes
  and links are in the DRM ledger. Both seats were restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes only the Car cell; strict linear
  scanout, VDI guest readiness, the remaining matrix, and WL-UX-009 readiness
  remain open.
- Current candidate Files matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Files Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the complete node-action inventory, peer/status lanes, file list,
  preview boundary, and transfer status remain readable; restore secure seat
  state after capture and accept only inspected frames.
- Current candidate Files matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Files Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. All frames show the
  complete ten-action node inventory, peer/status lanes, file list, preview
  boundary, and transfer/status strip without clipping or overlap; hashes and
  links are in the DRM ledger. Both seats were restored to payload `20955383…`,
  secure Dark/Construct/Default/Normal, active service, zero restarts, and no
  proof drop-in. This closes only the exact-candidate Files slice; strict
  linear scanout, VDI guest readiness, the remaining matrix, and WL-UX-009
  readiness remain open.
- Current candidate Editor matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Editor Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm direct-entry sidebar collapse, shared Editor identity, internal
  menu/toolbar reachability, document/status/details geometry, and taskbar
  contrast; restore secure seat state and accept only inspected frames.
- Current candidate Editor matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Editor Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. Direct-entry
  sidebars are collapsed and the shared Editor identity, menu/toolbar,
  document body, status row, details rail, and taskbar remain bounded; hashes
  and links are in the DRM ledger. Both seats were restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes only the exact-candidate Editor
  slice; strict linear scanout, VDI guest readiness, the remaining matrix, and
  WL-UX-009 readiness remain open.
- Current candidate Terminal matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Terminal Dark desktop,
  Dark narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the complete TERMINAL identity, menu/session controls, shell body,
  taskbar contrast, and bounded narrow layout; restore secure seat state and
  accept only inspected frames.
- Current candidate Terminal matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Terminal Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. The complete
  `TERMINAL` identity, menu/session controls, shell body, taskbar, and bounded
  narrow layout remain readable; hashes and links are in the DRM ledger. Both
  seats were restored to payload `20955383…`, secure Dark/Construct/Default/
  Normal, active service, zero restarts, and no proof drop-in. This closes only
  the exact-candidate Terminal slice; strict linear scanout, VDI guest
  readiness, the remaining matrix, and WL-UX-009 readiness remain open.
- Current candidate This Node matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for This Node Dark desktop,
  Dark narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the unified node navigation, health-score/status hierarchy, device
  and local-operations body, large-text scroll boundary, and taskbar remain
  readable and bounded; restore secure seat state and accept only inspected
  frames.
- Current candidate This Node matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed This Node Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. Unified node
  navigation, status/health hierarchy, device/local-operations body,
  large-text continuation, and taskbar remain readable and bounded; hashes and
  links are in the DRM ledger. Both seats were restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes only the exact-candidate This
  Node slice; strict linear scanout, VDI guest readiness, the remaining matrix,
  and WL-UX-009 readiness remain open.
- Current candidate Phones matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Phones Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the shared title/header, pairing status, tabs, feature and
  remote-input controls, and bounded large-text body; restore secure seat state
  and accept only inspected frames.
- Current candidate Phones matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Phones Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. The shared title,
  pairing state, tabs, feature and remote-input controls, and large-text body
  remain readable and bounded; hashes and links are in the DRM ledger. Both
  seats were restored to payload `20955383…`, secure Dark/Construct/Default/
  Normal, active service, zero restarts, and no proof drop-in. This closes only
  the exact-candidate Phones slice; strict linear scanout, VDI guest readiness,
  the remaining matrix, and WL-UX-009 readiness remain open.
- Current candidate Media matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Media Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the shared MEDIA identity/menu, source tabs, local/Jellyfin controls,
  honest empty-source state, and taskbar remain readable and bounded; restore
  secure seat state and accept only inspected frames.
- Current candidate Media matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Media Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. The shared MEDIA
  identity/menu, source tabs, local/Jellyfin controls, honest empty-source
  state, and taskbar remain readable and bounded; hashes and links are in the
  DRM ledger. Both seats were restored to payload `20955383…`, secure
  Dark/Construct/Default/Normal, active service, zero restarts, and no proof
  drop-in. This closes only the exact-candidate Media slice; strict linear
  scanout, VDI guest readiness, the remaining matrix, and WL-UX-009 readiness
  remain open.
- Current candidate Maps matrix recapture (2026-08-02): validate exact release
  `ae51c124…` on `.138` and Dell `.225` for Maps Dark desktop, Dark narrow
  (`800` logical width), Light desktop, and Light/Largest narrow. Confirm the
  governed map-content palette, health/alert rail, FAB lane, empty-state panel,
  and taskbar remain readable and separated; restore secure seat state and
  accept only inspected frames.
- Current candidate Maps matrix finding (2026-08-02): Dell `.225` Maps
  Light/Largest narrow (`800` logical width) remains rejected because native
  inspection shows the lower red alert pill clipped at the application
  viewport boundary above the taskbar. Evidence is recorded in the DRM
  ledger at `docs/platform/WL-UX-009-DRM-EVIDENCE-2026-07-31.md` with proof
  SHA `cc3c01404c1d96ace43d2cc68cb2fa843339a58b31f8144b545750497f3ddd96`.
  Keep this item open for a layout fix and a fresh direct-DRM recapture; do
  not claim Maps matrix closure or WL-UX-009 readiness.
- Maps alert-stack remediation (2026-08-02): update the Drive HUD's alert
  placement to reserve a bottom-safe viewport margin before painting multiple
  large-text status pills. Add geometry coverage for the Dell narrow profile,
  then rebuild and recapture the rejected `.225` Light/Largest Maps cell on
  direct DRM before changing its worklist disposition. Resolution evidence:
  focused Maps tests pass 274/274; candidate `b75e395a…` was recaptured on
  Dell direct DRM as `a52e044e00e6ba7cc4e305d7b97b8263e45f851b42817431a46288130c2f3b1a`.
  The two large-text status pills are fully visible above the taskbar, and the
  redundant no-data card is suppressed only in the combined no-fix/offline-
  blocked state. The seat was restored to the secure baseline. This resolves
  the recorded clipping finding; strict linear scanout, the remaining route /
  profile matrix, VDI guest readiness, and overall WL-UX-009 readiness remain
  open.
- Build-integrity blocker resolved (2026-08-02): reconciled the This Node
  `show_section_detail` call/definition after the application continuity slice;
  the focused BigBoy suite now compiles and passes 44/44. Live Maps proof
  remains blocked independently by the recorded narrow-layout finding below.
- Local services continuity update (2026-08-02): This Node's Services detail
  now folds a fixed, read-only local systemd failure provider alongside the
  existing mesh-published daemon health. The provider runs off the render
  thread, caps output at 32 unit names, treats systemd absence/refusal as an
  explicit unavailable state, and keeps restart behind the typed Actions
  confirmation/audit/recovery boundary. The focused BigBoy This Node suite
  passes 40/40; physical GUI recapture remains open.
- Printers/peripherals continuity update (2026-08-02): the durable Printers &
  Peripherals route now consumes a fixed, read-only local CUPS `lpstat` probe,
  bounded to 16 sanitized printer names plus local status/default evidence.
  Missing or refused CUPS remains explicitly unavailable; printer jobs,
  queues, USB authorization, and dock mutation remain outside the route until
  typed confirmation/audit/recovery providers exist. The focused BigBoy This
  Node suite passes 41/41; physical peripheral proof remains hardware-gated.
- Firewall posture continuity update (2026-08-02): Security & Privacy now
  consumes a fixed, off-render-thread firewalld `--state` observation and
  distinguishes running, not-running, unavailable, and unknown-provider
  states. Zone/rule detail, encryption, broader security policy, and firewall
  mutation remain explicitly unavailable; the UI does not infer a general
  security posture from one firewalld probe. The focused BigBoy This Node suite
  passes 42/42; physical security-policy evidence remains open.
- Remote-access continuity update (2026-08-02): Virtualization & Remote Access
  now reuses the durable System Remote Proofing policy and derived
  Sunshine/Moonlight/VNC service plan in the This Node detail route. It exposes
  bounded enablement, bind scope, firewall policy, capture/encoder, frame
  target, local approval, indicator, input, fallback, and provider warnings;
  lifecycle and trusted-session mutations remain owned by the existing System/
  VDI authorities. Catalog tests pass 9/9 and focused This Node tests pass
  42/42 on BigBoy.
- Backup posture continuity update (2026-08-02): Backup & Restore now reads
  metadata for the existing encrypted `state-backup.enc` artifact at the
  canonical workgroup/node/mackesd path. This Node reports bounded presence,
  size, modification time, missing, and invalid/symlink states without opening
  or exposing encrypted contents. Passphrase verification and restore remain
  privileged mackesd operations outside the UI. Catalog tests pass 9/9 and
  focused This Node tests pass 43/43 on BigBoy.
- Applications continuity update (2026-08-02): Services & Applications now
  reads the existing bounded `apps-installed.json` and `running-apps.json`
  mirrors under the canonical workgroup/node directory and exposes aggregate
  installed and running counts. Missing, malformed, symlinked, oversized, or
  unavailable mirrors remain explicit unknown/unavailable states; app names,
  launch, and mutation continue through the existing Front Door authority.
  Focused This Node tests pass 44/44 on BigBoy; physical application proof
  remains open.
- Encryption posture continuity update (2026-08-02): Security & Privacy now
  performs an off-render-thread, fixed-root observation of `/sys/class/block`
  device-mapper entries and counts only mappings whose local `dm/uuid` begins
  with `CRYPT-LUKS`. The route reports no mappings, observed encrypted versus
  total mappings, or an explicit provider failure; it never exposes mapping
  names, paths, keys, unlocked state, or a full-disk-encryption claim. Hostile
  fixture coverage passes in the focused This Node suite, now 46/46 on BigBoy.
  Encryption policy and mutation remain provider-gated; physical security
  evidence remains open.
- Security copy truthfulness update (2026-08-02): corrected the mesh-level
  Security & Privacy summary so it no longer contradicts the trusted local
  encryption/firewalld cards. It now distinguishes snapshot-wide policy, local
  observations, camera-permission gaps, and partial-fact limitations. The
  focused This Node suite remains green at 46/46 on BigBoy.
- Physical-evidence audit update (2026-08-02): inspected the recorded direct-DRM
  This Node Dark desktop and Light/Largest narrow frames for hierarchy, health
  glyphs, Inventory/Actions navigation, contrast, taskbar touch targets, and
  scroll continuation. They remain usable layout evidence, but their exact
  candidate predates the later application, encryption, and Security copy
  changes; do not treat those hashes as proof of the newest payload. A fresh
  direct-DRM recapture is required after the next accepted release candidate.
- Local security freshness update (2026-08-02): Security & Privacy now gives
  firewalld and encryption observations independent `Fresh`, `Stale`, or
  `Awaiting local provider` badges. A successful local observation ages out
  after 45 seconds, so a hung or silent worker cannot leave old security facts
  looking current. Focused BigBoy This Node tests pass 46/46; the existing
  mesh-wide health authority remains unchanged.
- Local-provider freshness continuity update (2026-08-02): the same bounded
  freshness badge now appears on local Services, Printers & Peripherals, Backup
  & Restore, and Services & Applications cards. Each successful off-render-
  thread response records its own observation age; a silent worker becomes
  `Stale` after 45 seconds and never masquerades as current. Provider errors
  and not-yet-seen states remain distinct. Focused BigBoy This Node tests pass
  46/46.
- Diagnostics freshness continuity update (2026-08-02): the bounded redacted
  journal provider now shows its own `Fresh`, `Stale`, or `Awaiting local
  provider` badge. A stopped journal worker therefore cannot leave old warning
  and error lines looking current, while the fixed query, redaction, size cap,
  and no-user-query boundary remain unchanged. Focused BigBoy This Node tests
  pass 46/46.
- Hardware-provider freshness update (2026-08-02): the trusted `mde-seat`
  HardwareStatus contract now carries a bounded observation timestamp. Hardware
  detail renders `Fresh`, `Stale`, or `Awaiting provider timestamp` for local
  thermal/fan, storage, firmware, dock, and platform-profile evidence instead
  of relying solely on mesh snapshot age. No paths or mutation verbs cross the
  seam. BigBoy `mde-seat` hardware tests pass 3/3 and focused This Node tests
  pass 46/46.
- Recovery & Reset route continuity update (2026-08-02): This Node now indexes
  a distinct Recovery & Reset hierarchy/search route with a full-page boundary
  that names the privileged provider required for recovery-environment, reset,
  rollback, and destructive restoration controls. The page keeps encrypted
  backup metadata and passphrase-gated `mackesd` verification/restore as the
  existing safe continuity path, and presents no reset action as available.
  Catalog tests pass 9/9 and focused This Node tests pass 46/46 on BigBoy.
- Time/language/region provider update (2026-08-02): the durable Time, Language
  & Region route now consumes a bounded local provider for host locale/language
  values from fixed `/etc/locale.conf` or `/etc/default/locale` files and the
  host time zone from fixed timezone evidence. Values are sanitized, kept
  read-only, and show `Fresh`, `Stale`, or an explicit provider error; display
  clock preference remains owned by the typed System provider, while locale
  mutation, keyboard-region policy, and time synchronization remain gated.
  Focused This Node tests pass 47/47 on BigBoy.
- Full-page responsive evidence update (2026-08-02): the governed detail-route
  render fixture now mounts the typed System provider instead of testing every
  route only through a provider-less fallback, and renders every indexed page
  again at 520px logical width with 1.4x text scale. This covers the real
  locale, personalization, virtualization, and OS-management detail branches
  alongside the device pages. Focused This Node tests pass 47/47 on BigBoy.
- Physical-evidence audit update (2026-08-02): the available Dell proof target
  `172.20.146.225` accepts SSH but currently reports an inactive
  `mde-shell-egui.service` and no `/dev/dri/card0`; the `.138` proof target is
  not reachable on SSH. The checked-in This Node PNGs remain useful visual
  layout evidence, but are not current-payload or physical-control proof.
  Fresh direct-DRM capture and reachable-device action evidence remain open
  until a live DRM seat is available.
- Time-sync and keyboard-region continuity update (2026-08-02): the local
  Time, Language & Region provider now also reads bounded keyboard-region facts
  from fixed host configuration and the fixed `timedatectl` synchronization
  posture. The UI distinguishes synchronized, not synchronized, not reported,
  and provider-error states; it exposes no host mutation or false sync claim.
  Focused This Node tests pass 47/47 on BigBoy.
- Current-payload route validation (2026-08-02): full production-feature
  candidate `2f32f935c92a4cf84f926221a093a3666638fee9063ec4f9a8dc8ef1f686f628`
  was built on BigBoy with `drm,live-vdi,media-mpv`. On `.138`, Music Dark
  desktop, Music Dark narrow (`800` logical), and Music Light/Largest narrow
  were visually inspected and accepted; Media Dark desktop was also captured
  and accepted. Evidence and native readback hashes are recorded in the DRM
  ledger. `.138` was restored to payload `20955383…`, secure login-at-boot,
  active service, and zero restarts. This closes only the current-payload
  Music slice plus one Media cell; the remaining Media profiles, Phones,
  Terminal, Editor, Browser boundary, strict linear scanout, Dell adoption,
  and WL-UX-009 readiness remain open.
- Current-payload route validation continuation (2026-08-02): the same
  production-feature candidate `2f32f935…` was explicitly routed on `.138` to
  Phones, Terminal, and the unified Editor/Communications surface. Dark
  desktop frames were visually inspected and accepted for all three. The
  Editor frame records the approved boundary: Construct owns the Mesh Teams
  host frame and embedded Editor surface; no guest application styling is
  claimed. `.138` was restored to payload `20955383…`, secure login-at-boot,
  active service, and zero restarts. Remaining Light/Largest and narrow cells,
  Dell adoption, Browser/VDI boundaries, strict linear scanout, and overall
  WL-UX-009 readiness remain open.
- Dell adoption validation (2026-08-02): the production-feature candidate
  `2f32f935…` was installed on Dell `.225` and the Phones, Terminal, and
  unified Editor/Communications Dark desktop frames were visually inspected
  and accepted. The Editor frame preserves the approved Construct-owned host
  and embedded-editor boundary. Dell was restored to payload `20955383…`,
  secure login-at-boot, active service, and zero restarts. This closes only
  the Dell Dark desktop route slice; Light/Largest and narrow cells, remaining
  Media coverage, Browser/VDI boundaries, strict linear scanout, and overall
  WL-UX-009 readiness remain open.
- Superseded inventory-health update (2026-08-02, retired 2026-08-03): the
  inventory landing's global score, local freshness rollup, and provider badges
  were removed by the System and Mesh Health cutover. Provider evidence still
  carries freshness, but issue presentation and A–F grading exist only in the
  centered modal.
- Superseded critical-alert update (2026-08-02, retired 2026-08-03): the linked
  This Node AlertInbox and inline health recovery card were removed. Typed
  conditions, acknowledgement, snooze, and guided recovery now live only in
  System and Mesh Health; signed mesh Chat remains notification transport and
  is not a second health ledger.
- NetworkManager SecretAgent boundary update (2026-08-02): `mde-seat` now
  provides an in-process, non-persistent NetworkManager SecretAgent lifecycle
  and a typed profile-activation method using only validated provider-issued
  profile/device object paths. Secret values exist only in the callback and
  D-Bus activation exchange; they are never serialized, logged, or exposed to
  This Node snapshots. Save/delete persistence is refused, malformed metadata
  is rejected, and mesh routes/DNS are not rewritten by activation. The This
  Node profile action remains fail-closed until a trusted-session responder is
  mounted by the shell. BigBoy `mde-seat` network tests pass 8/8 and focused
  This Node tests pass 48/48.
- Phones large-text remediation (2026-08-02): moved the always-available
  `Disarm now` control into the wrapped arm-action lane, removing the needless
  extra row at large text. Phones tests pass 26/26. Candidate
  `cc56fdf0466a29506b8b7adcf27af8aa3f7a034d87bdebfe20143093289f2dbc` was
  recaptured on Dell `.225` Light/Largest narrow; the complete Remote input
  card now ends above the taskbar and is accepted. Dell was restored to
  payload `20955383…`, secure login-at-boot, active service, and zero restarts.
  Remaining route/profile cells, Browser/VDI boundaries, strict linear
  scanout, and overall WL-UX-009 readiness remain open.
- Dell Editor profile validation (2026-08-02): candidate `cc56fdf0…` was
  explicitly routed to the unified Editor/Communications surface on Dell
  `.225`. Dark narrow, Light desktop, and Light/Largest narrow frames were
  visually inspected and accepted. The approved host-owned Mesh Teams and
  embedded Editor boundary remains clear at each profile. Dell was restored to
  payload `20955383…`, secure login-at-boot, active service, and zero restarts.
  Remaining Media profiles, Browser/VDI boundaries, strict linear scanout, and
  overall WL-UX-009 readiness remain open.
- Live-render finding (2026-08-02): Dell `.225` Media Light/Largest narrow
  current-payload proof clips the Jellyfin empty-state line at the taskbar
  boundary. Reject the cell; keep the status copy truthful and make the Media
  content lane taskbar-safe before recapturing the exact candidate.
- Dell Terminal profile validation (2026-08-02): candidate `cc56fdf0…` was
  explicitly routed on Dell `.225` and Dark narrow, Light desktop, and
  Light/Largest narrow frames were visually inspected and accepted. Terminal
  content remains bounded above the taskbar in all three profiles. Dell was
  restored to payload `20955383…`, secure login-at-boot, active service, and
  zero restarts. Remaining Editor/Media profiles, Browser/VDI boundaries,
  strict linear scanout, and overall WL-UX-009 readiness remain open.
- Network profile activation continuation (2026-08-02): This Node/System now
  mounts the in-process NetworkManager SecretAgent only while the trusted
  session is viewing the relevant action surface. Profile activation uses
  typed, provider-issued profile/device object paths, requires a visible
  second confirmation, and collects credentials through an ephemeral modal;
  secrets are not persisted, placed in snapshots, or written to audit output.
  APN/DNS/proxy edits and imported VPN mutation remain unavailable, and the
  action warns that activation may interrupt underlay/mesh reachability.
  BigBoy bridge tests pass 1/1; the existing BigBoy `mde-seat` network suite
  passes 8/8 and focused This Node tests pass 48/48. Recovery/reset, typed
  local service restart, remaining provider gaps, and physical DRM evidence
  remain open.
- Typed service-control continuation (2026-08-02): the existing bounded failed
  systemd-service observation now has a matching `mde-seat` D-Bus provider.
  This Node Actions offers only provider-reported, validated `.service` units;
  the operator must arm and confirm the exact unit, and systemd resolves and
  restarts it without a shell fallback. Provider refusal, absent system D-Bus,
  malformed targets, and stale projections remain visible as unavailable or
  refused; audit output contains only the fixed action label, outcome, and
  timestamp. BigBoy service-provider tests pass 4/4 and focused This Node tests
  pass 48/48. Recovery/reset, update application, remaining provider gaps, and
  physical DRM evidence remain open.
