# Drain reconciliation ledger — 2026-07-19 (authoritative)

Source: 8-agent reconciliation workflow (`wf_924f2a46-283`, 929k tokens, 358 tool-uses, file:line evidence per epic) run against branch `agent/browser-enterprise-hardening` @ `b999251e`. This ledger is the drain map; per-epic `Status:` lines in WORKLIST.md defer to it where they disagree ([[worklist-drift-reconcile-first]]).

## Tally

| Disposition | Count | Meaning |
|---|---|---|
| ✅ Done (marker flip) | 8 | already complete; evidence below |
| 🔄 Draining now | 4 | farm agents in flight |
| 🟡 Drainable-open | 12 | autonomous, scoped |
| ❓ Needs operator decision | 3 | unmade design decision gates it |
| ⛔ Park-blocked | 16 | operator/hardware/live gate |
| **Total** | **43** | of 43 epics |

**Bottom line:** of 43 epics, **8 are already done**, **4 are draining now**, **12 more are autonomously drainable** (a few carry a live-seat proof I can run on the authorized .15 seat). **19** (3 need an unmade design decision + 16 park-blocked) cannot be drained without the operator — physical hardware, a live multi-node fleet, or signing/release authority. **Autonomous ceiling = 24/43 code-complete; the last 19 need you.**

## ✅ Mark-done — verified complete on real code paths (marker lagged)  (8)

### WL-ARCH-005 — Browser worker crypto seam and mde-seal emitter completion
*group:* `cloud-arch` · *actual:* **mostly-done**

<details><summary>evidence</summary>

Shared crypto seam COMPLETE and reused by all 3 consumers: mde-seal/src/lib.rs has real Argon2id+XChaCha20-Poly1305 seal_bytes (lib.rs:102) / unseal_bytes (lib.rs:138); ca/backup.rs:62-63 re-exports mde_seal (delegates, no dup), ipc/secret_store.rs:54 uses mde_seal::age_key_path, browser_passkeys.rs:264/1229/1251 uses age_key_path/seal_bytes/unseal_bytes. NO duplicate crypto — grep for argon2/XChaCha/aead in crates/mesh/mde-browser-workers returned nothing. Browser passkey worker is registered+spawned (worker_role.rs:213 rank 1; spawn.rs:2217). NO production placeholder returns. The only residue is TEST-ONLY: emit_pinned_vectors is a #[ignore] vector regenerator (lib.rs:356-364) and the `== "__EMIT__"` guards (lib.rs:368-370, 384-386) are dead branches because PIN_SEALED_HEX/PIN_DERIVE_KEY_HEX are populated with real hex (lib.rs:351/379-380).

</details>

### WL-CRIT-002 — VDI reconnect and disconnected-state UX
*group:* `vdi-media` · *actual:* **done**

<details><summary>evidence</summary>

vdi/mod.rs:963-1099 defines SessionPhase (Live/Reconnecting{attempt}/Failed) with capped-exponential reconnect_backoff (0.5s→8s) and session_overlay(Retry+PickDifferent); :1412-1478 on_transport_drop/note_live_frame/poll_reconnect drive the bounded ladder; poll_live_rdp/vnc/spice (:1620-1739) take the dead handle on Error/Ended, call publish_broker_disconnect_if_active + on_transport_drop; forward_input (:2131-2144) gates on has_live (handle=None after drop) so no input goes into a dead channel; overlay painted + actions wired at :1951-1957 (retry_now/clear_target); Connected→publish_broker_active restores Active. Unit tests tests.rs:244-396 (drop ladder→Failed@max, capped backoff, redial→frame→Live, overlay faces, user-close never reconnects).

</details>

### WL-CRIT-005 — Substrate-v2 fleet cutover and LizardFS wedge removal
*group:* `crit-run` · *actual:* **mostly-done**

<details><summary>evidence</summary>

LizardFS rip-out landed: commit 81d18bd2 'SUBSTRATE-6: rip out the dead LizardFS plane (one-way)'. Cutover helpers ship in this branch + RPM: install-helpers/{cutover-substrate-v2,setup-etcd,setup-syncthing}.sh packaged to /usr/libexec/mackesd/ (crates/mesh/mackesd/Cargo.toml:483-510). mesh_mount.rs is now an sshfs/FUSE media seam, not LizardFS. All 38 remaining lizardfs/mfsmount refs are ENFORCEMENT GUARDS/tests asserting its absence (install-helpers/lint-shared-substrate.sh:33; automation/{promotion,testbed} 'no FUSE/LizardFS mounts' checks). Runbook docs/ops/substrate-v2-cutover-runbook.md header: 'HISTORICAL - the cutover is COMPLETE'. Live fleet was rebuilt fresh on substrate-v2 (memory: fleet-rebuild-2lh; gate-execution-11.2.0 'SUBSTRATE-V2 cutover = no-op').

</details>

### WL-FUNC-004 — Browser power tools, downloads, PDF/print, capture, and protocol handling
*group:* `browser-func` · *actual:* **done**

<details><summary>evidence</summary>

330 MenuAction variants all dispatch to real methods (menubar.rs:1249 OpenViewSource->open_active_view_source, 1250 OpenChromiumDevtools->open_chromium_devtools, gated by engine 463). Real backends: CUPS printing (web/printing.rs:142-158 lpstat -e/lp), protocol handlers mailto->email/magnet->transfers (browser_protocol.rs:291, refuses to fake), capture.rs (62KB), downloads dispatch REAL shared Transfer jobs (mod.rs:9080 browser_output_transfer_job->TransferJob, 1697 shared Transfers client, test 25350 browser_download_manager_filters_and_dispatches_shared_transfer_jobs). No todo!/unimplemented!/stub in web/ (only anti-placeholder guards). Disabled-explain-gate landed (browser_options_disabled_rows_explain_their_command_gate). Live .15 proof: installed-RPM CEF download + Google/News PDF-path smokes with painted frames (worklist evidence 1166-1194).

</details>

### WL-PERF-001 — VDI dirty-rectangle display uploads
*group:* `vdi-media` · *actual:* **done**

<details><summary>evidence</summary>

mde-vdi-core/src/damage.rs defines DamageRect/DamageLog/FrameDamage + sub_color_image (the ImageDelta slice math); VNC produces exact per-rect damage (vnc/session.rs:207 damage.push(DamageRect)), RDP likewise (rdp/session.rs:139), SPICE honestly marks full (spice/session.rs:131 mark_full); shell uploads only damaged sub-rects with a size-guarded full fallback (vdi/mod.rs:1843-1859 set_partial else set). Unit tests: vnc apply_rect_reports_its_rectangle_as_damage, spice frames_report_full_damage, and sub_color_image pixel-identity.

</details>

### WL-PERF-003 — Browser native-grade frame rate, occlusion, and audio
*group:* `browser-perf-ux` · *actual:* **mostly-done**

<details><summary>evidence</summary>

All 6 cited commits present on this branch (d818c975/233154ba/c735f0ee/4b9cdd2c/ed56d8db/fa16eddf, git log). P0/P1/P2a/P3-engine are real code, not stubs: wire SetHidden tag 38 encode/decode (crates/desktop/mde-web-wire/src/lib.rs:666,1229); engine WasHidden @offset 312 with on-seat cross-check comment + live vtable call (crates/desktop/mde-web-cef/src/cef_browser/mod.rs:366 CEF_BROWSER_HOST_WAS_HIDDEN_OFFSET=312, :4651 was_hidden(), :1257 dispatch); shell edge-triggered reconcile_tab_visibility drives set_hidden (crates/desktop/mde-shell-egui/src/web/mod.rs:2748,2761,9655). P3 audio is REAL but env-gated + default-OFF: wants_native_audio reads MDE_CEF_NATIVE_AUDIO (mod.rs:3989) and get_audio_parameters returns 0 only in that mode (mod.rs:3999-4008); default path still returns 1 (indicator, silences OS output). So Required-outcome dimensions 1-3 (fg 60fps / hidden ~0fps / 5-video occlusi …

</details>

### WL-RUN-001 — Auto-repair must either repair or say observe-only
*group:* `crit-run` · *actual:* **done**

<details><summary>evidence</summary>

Real take-action executor implemented + wired (commit ef8f8350 'make reconcile auto-repair real for the safe subset (mackesd-03)'). crates/mesh/mackesd/src/worker.rs: apply_safe_repairs() (line 695) re-probes the incident peer's overlay (probe_rtt -> Nebula re-establishes the dropped hole-punch = real self-heal), bounded by DEFAULT_MAX_REPAIRS_PER_TICK, called live in the tick at line 351, gated by load_repair_policy() (line 283, config auto_repair master switch). Observe-only cases are honestly recorded: RepairOutcome::{ManualRepairRequired,DeferredCapReached,Disabled} -> audit action tokens manual_repair_required/observe_only_disabled (lines 633-643). Tests with injected FakeProber + audit assertions at worker.rs:1402,1435.

</details>

### WL-RUN-005 — Device Manager multi-source inventory and fault notifications
*group:* `crit-run` · *actual:* **done**

<details><summary>evidence</summary>

Multi-source inventory: 5 HostKinds (Node/Nova/Phone/Lan/Router) each synthesized with honest-partial categories in crates/desktop/mde-shell-egui/src/device_manager/mod.rs (nova_host->virtio :962-1030, phone_host->Radios :1046-1090, lan_host->Network adapters :1097-1120, router_host->Network/System/Firmware :1141-1190), each with inline test assertions; tools gated so verbs are hidden on non-PC hosts. Fault detector + debounced notify FULLY implemented in crates/mesh/mackesd/src/workers/device_inventory.rs (DEVMGR-9): fault_transitions() edge-detects entry-into-problem (:1093), DeviceFaultGate/FAULT_COOLDOWN debounce flapping (:1127), emit_fault_alert() -> event/notify/device-fault -> chat worker folds into alert:<self> -> Chat+phone (:1155-1184). Wired live in hardware_probe.rs:219,232,269,281. Tests: fault_transitions_fire_only_on_entry_into_a_problem_state (:1547), fault_gate_debounce …

</details>

## 🔄 Draining now — farm wave dispatched 2026-07-19  (4)

### WL-BUILD-003 — Promotion rollback, version matrix, and secret-scan gates
*group:* `build` · *actual:* **partial**

**Remaining:** Add a promotion rollback/downgrade verb (previous-NEVRA re-install) + a non-production rollback drill + rollback runbook doc. Optionally broaden lint-worklist secret-scan repo-wide and wire it into CI/farm (deferred by operator). Fedora matrix already documented.

<details><summary>evidence</summary>

Mixed: one criterion done, one open, one deferred. DONE (Fedora matrix): docs/BUILD-ENVIRONMENT.md:423-484 '§7 Fedora target matrix & the glibc compatibility contract' documents bootc base F42 (glibc floor), canonical container RPM F43 default, farm native F42, CI/workstation F44, with the glibc-forward-compat rule. OPEN (rollback): automation/promotion/mcnf-promotion-cycle.sh:704-723 exposes no rollback/downgrade verb; :662-686 promote_do is forward-only (rpm -Uvh --replacepkgs); grep for rollback|downgrade|--oldpackage across automation/ + docs/ops/ returns nothing (no promotion rollback runbook; docs/ops/promotion-pipeline.md's 'version matrix' at :35 is runtime host-drift reporting, not a rollback flow). PARTIAL+DEFERRED (secret-scan): install-helpers/lint-worklist.sh:71-95 secret_check greps only WORKLIST.md for DO/AWS/age/private-key/mm-path shapes (self-test :210-212), plus per-fi …

</details>

### WL-FUNC-003 — Browser mesh sync, follow-me tabs, and bookmark integration
*group:* `browser-func` · *actual:* **mostly-done**

**Remaining:** Add the explicit deterministic two-store convergence fixture the verification method calls for (per-end round-trips are tested; the cross-node hop is not fixture-proven); confirm follow-me tabs are the intended session-restore behavior (no distinct real-time follow-me exists). Session-state conflicts are Syncthing LWW, not CRDT — acceptable but worth noting.

<details><summary>evidence</summary>

Browser uses the SYSTEM bookmark manager as truth with NO competing store: web/mod.rs:751-755 mirrors state/bookmarks/collection (mde_bookmarks::Collection) into bar links (788 bookmark_bar_links_from, 3561-3592 fold). mde-bookmarks is a real CRDT with proven convergence: services/mde-bookmarks/src/crdt.rs:524 concurrent_edits_from_two_nodes_converge_regardless_of_order, 598 hlc-stamped converge. Session sync is wired end-to-end: shell publishes action/browser/session-sync carrying tabs+settings(speed_dial,zoom,power,vertical)+downloads (mod.rs:2339 publish, test 19318), restores it (3315 restore_session_sync_snapshot, startup 3455); the mesh worker (mde-browser-workers/src/browser_session_sync.rs:1-10) mirrors the exact snapshot JSON into the Syncthing workgroup root + materializes send-tab outbox (175-303) with phone delivery. Send-tab incoming poll consumed (mod.rs:2515).

</details>

### WL-PERF-002 — Seat responsiveness residuals
*group:* `vdi-media` · *actual:* **partial**

**Remaining:** Add a per-frame repaint while vdi.has_live_transport() (mirror the media is_playing repaint at main.rs:1294) so live VDI frames wake the idle seat; then a live seat proof for non-Browser media/VDI wake + slow-probe non-stall.

<details><summary>evidence</summary>

Slow-probe isolation done: seat_pump.rs moves the blocking ddcutil/PipeWire seat probes to a background thread and wakes the render thread via ctx.request_repaint() on publish (:230); DDC detect cached on connector-set change, getvcp on a slow cadence. Media frame source done: main.rs:1294-1296 requests a repaint while self.media.is_playing(). Browser done per archived WL-CRIT-003 + recent heartbeat work. GAP — VDI: the Desktop surface handler (main.rs:1240-1256) and vdi_panel (vdi/mod.rs:1869-1957) contain NO request_repaint while a live transport is active; LiveRdpHandle::spawn (:507-546) carries no egui Context to wake from the frame thread; has_live_transport is referenced only inside the vdi module. So a live brokered desktop only repaints on input or the 5s chrome heartbeat (chrome.rs:327 request_repaint_after(REFRESH)) — i.e. it will not reliably wake the seat without pointer move …

</details>

### WL-RUN-002 — Failure-rate metrics and process-wide counters
*group:* `crit-run` · *actual:* **partial**

**Remaining:** Add process-wide counters incremented by producers for reconcile failures, drift events, and Bus publish errors, render them via the exporter with stable names, and add the acceptance-required reconcile-failure unit test (the operator's 'worker-restart counters first' slice is already done). Pure code + tests; only a soft metric-naming review dependency.

<details><summary>evidence</summary>

Worker-restart + breaker-trip counters DONE and live-incremented at the real supervisor sites: crates/mesh/mackesd/src/workers/mod.rs:1242 (w.restarts += 1) and :1408 (w.breaker_trips += 1); rendered as mackesd_worker_restarts_total{worker=} + mackesd_breaker_trips_total{worker=} by metrics_exporter.rs:417-431; tested at metrics_exporter.rs:725-737 and mod.rs:1694 (status_map_tracks_lifecycle_and_restarts). metrics.rs provides Counter/Histogram + atomic write_textfile. MISSING: no reconcile-failure counter, no drift-event counter, no bus-publish-error counter incremented in production paths. The only mackesd_drift_detected_total reference is a render-example inside a metrics.rs unit test (line 277), not a live-incremented producer. fleet_reconcile.rs failure path only tracing::warn (no counter). No process-wide counter registry beyond the supervisor status map.

</details>

## 🟡 Autonomously drainable — scoped, not yet dispatched  (12)

### WL-ARCH-003 — Shared Bus/Persist client seam and wire-contract fixtures
*group:* `cloud-arch` · *actual:* **partial**

**Remaining:** Migrate the remaining reader surfaces (web/, storage/, chat/, explorer/, vdi/, device_manager/, session_rail) onto BusReader; add latest-value read helpers to the seam per its own note; build the shared wire-contract fixtures covering mirrored wire types; run a poll-heavy performance trace. All pure in-repo refactor + tests, no external gate.

<details><summary>evidence</summary>

Shared seam LANDED: crates/desktop/mde-shell-egui/src/bus_reader.rs BusReader (arch-11) with new/client/open (bus_reader.rs:42/50/59); ~68 refs across shell, first wave migrated (discovery/services_flow/host_mirror/formfactor/spawn_lighthouse_flow/mesh_view/datacenter/timers/front_door_peer_apps + phones_hub/iac/cloud_plane). BUT seam is OPENER-ONLY — no read/latest-value helpers ('room to grow read helpers later', bus_reader.rs) — and it EXPLICITLY opens per call rather than caching a Connection (bus_reader.rs:10-18, for the BUS-INODE-ORPHAN self-heal), so AC1 'no longer open SQLite per tick' is not literally met (relies on perf-3 cheap-open). Migration incomplete: reader sites still direct-open in session_rail.rs:195, web/mpris.rs:355/377, storage/mod.rs:1564, web/content_tools.rs:378, vdi/mod.rs:213, device_manager/mod.rs:2668, chat/mod.rs:2790, explorer/mod.rs:664, web/mod.rs:2370. N …

</details>

### WL-ARCH-004 — Mackesd worker registration, decomposition, and restart policy
*group:* `cloud-arch` · *actual:* **partial**

**Remaining:** Unify the two registries into one declarative table carrying name, role/capability, constructor, and restart policy — replacing the ~136 imperative spawn+push sites in run_serve; split large worker families behind stable traits. Pure in-repo refactor (no operator/infra gate) but Epic-sized across ~136 call sites; do as staged PRs per the epic's own dependency note.

<details><summary>evidence</summary>

Explicit restart policy + role-gate tests DONE: mackesd.rs:2482 imports RestartPolicy/Spawn/Supervisor, each worker registered with RestartPolicy::OnFailure (mackesd.rs:2403-2405, 2732, 2770); role-gate coverage worker_role.rs workers_for_rank:472 / workers_for_class:482 + drift-guard test worker_spawns_and_the_census_do_not_drift (worker_role.rs:502-520). BUT the single declarative registry is EXPLICITLY NOT built: registration is still ~136 imperative sup.spawn(...) / worker_names.push(...) sites in run_serve (worker_role.rs:506), split across TWO registries (static census WORKER_TIERS/WORKER_CAPABILITIES/NON_TIERED_WORKERS vs imperative spawns); worker_role.rs:518-520 states verbatim 'This does NOT unify the two registries — that is a larger refactor'. mde-worker-core Worker trait is name()+run() only (mde-worker-core/src/lib.rs:71-82) — carries no role/capability/constructor/restart- …

</details>

### WL-DOC-001 — Stale architecture/design docs need supersession banners
*group:* `docs-test-maps` · *actual:* **partial**

**Remaining:** Add supersession banners (or archive) to the ~20 unbannered design docs referencing retired arch; author a doc-supersession lint + historical-file allowlist to satisfy the stated verification method. Operator-facing docs already largely clean.

<details><summary>evidence</summary>

The 4 operator docs named in the header (WORKLIST.md:77-79) are effectively clean: the only retired-term hits are historical/corrective, not instructions — docs/ops/promotion-pipeline.md:91 ('Cosmic was terminated, mde-shell-egui.service was started') and docs/BUILD-ENVIRONMENT.md:275 ('legacy cosmic/iced leftover in the install line — no longer needed'). BUT the broad acceptance is unmet: ~20 docs/design/*.md still mention cosmic/iced/mde-workbench/cloud-hypervisor with NO supersession banner (e.g. docs/design/router-control.md, front-door.md, apps-launcher.md, motion-system.md, event-routing.md, e12-9-10-libvirt-rescope.md); only 39/101 design docs carry any historical/superseded marker. The verification method ('grep for retired terms with allowlisted historical files') has no enforcement: install-helpers/ has lint-brand-identity/bus-names/layered-tiers/shared-substrate/style-leaks/wo …

</details>

### WL-DOC-002 — Re-key operator queue to reconciled IDs
*group:* `docs-test-maps` · *actual:* **open**

**Remaining:** Re-key every NEEDS-OPERATOR entry to a WL-* id (or archive it with a disposition), and either merge the queue into WORKLIST.md's Blocked items or convert it to an archive/pointer so no old tracker is presented as active.

<details><summary>evidence</summary>

docs/NEEDS-OPERATOR.md is still a standalone active queue keyed to OLD IDs throughout: BUILD-PLATFORM-1/5/6 (lines 8-11), DATACENTER-3/23 (19-20), FED-RUNTIME/FED-XMESH/FED-GUI (26-28), LIGHTHOUSE-VARMOUNT (32), MEDIA-2..10 (36-41), OW-3/4/5/7/8/11/12 (56-71), E12-9 (46,74), DAR-19 (79). Its header still frames it as the live blocked-work queue (line 1-3). Only ONE item maps to a WL ID — NAMING-1 → WL-UX-004 (line 93). The header directive WORKLIST.md:80 ('merge docs/NEEDS-OPERATOR.md fully into this active worklist; it should not remain a separate queue') is not done: it remains a parallel tracker with old-only IDs.

</details>

### WL-DOC-003 — Active worklist stewardship
*group:* `docs-test-maps` · *actual:* **partial**

**Remaining:** Author the full stewardship lifecycle doc (ID scheme, required fields, archive-on-close procedure, evidence citation, duplicate-workstream avoidance) across the worklist header + AGENTS.md + a new docs/worklist-archive/README, and wire lint-worklist.sh into the documented process.

<details><summary>evidence</summary>

Dependency met: install-helpers/lint-worklist.sh exists and is robust — enforces WL-id headers, Status vocabulary (Remaining/Blocked/Needs clarification), no retired '- [ ]' checkbox markers, line-length, secret-shape, and @farm-payload validity, with a --self-test (lint-worklist.sh:15-230). Partial lifecycle prose lives in WORKLIST.md:1-26 (single active worklist, other trackers are evidence sources, move completed to archive with a disposition, status vocabulary). BUT the full lifecycle the epic requires is NOT documented anywhere: no WL-id minting scheme, required-fields list, how-to-cite-evidence, or how-to-avoid-duplicate-workstreams; docs/worklist-archive/ has NO README (only the 3 dated 2026-07-16 archive dumps); AGENTS.md (read in full) and AI_GOVERNANCE.md have no worklist-stewardship section and neither references lint-worklist.sh.

</details>

### WL-FUNC-006 — Bottom navigation session entries and file-operation progress
*group:* `browser-func` · *actual:* **mostly-done**

**Remaining:** The only open item per the epic's own evidence is a live .15 screenshot-level visual smoke of the bottom rail — capturable autonomously via the visual-audit harness on the authorized .15 seat.

<details><summary>evidence</summary>

VDI sessions render as switchable bottom-rail entries: session_rail.rs:1-6 projects action/vdi/session, entries() (71-88) shows non-closed sessions as SessionRailEntry, focus_session (94-103) switches. Shared file-operation progress: dock/mod.rs:34 FileOperationProgress + 803 set_file_operation_progress folds into status.segments.file_operations; mde-files-egui/src/model/mod.rs:1424-1480 operation_progress_summary folds local ops (copy/compress/extract) + transfers_jobs (upload/download) into one summary; main.rs:2881 shell_file_operation_progress also folds Browser downloads; click routes to Files->Transfers (main.rs:2890). Progress survives surface switches via the per-frame pump (main.rs:1924) + progress-pump slice. Farm screenshot artifacts (taskbar-file-progress-rail/panel.png) pulled and inspected.

</details>

### WL-FUNC-008 — Unified services view
*group:* `browser-perf-ux` · *actual:* **partial**

**Remaining:** Define unified ServiceRecord (source/provenance, endpoint, health, role, action-ownership); daemon aggregator merging the 3 sources with stale-entry TTL age-out; shell services view consuming it; fixture tests with mixed sources + live registry smoke. Essentially the whole deliverable is unbuilt; only the raw upstream sources exist.

<details><summary>evidence</summary>

The three service sources exist but are NOT unified into one provenance/health view. (1) Published services: service_directory::NodeServices per node, mirrored in crates/desktop/mde-shell-egui/src/phones_hub.rs:109 (a bare Vec<String> of service names, no health/provenance). (2) Probe-discovered services: probe_nmap worker writes probe-inventory.json + announces probe/changed (crates/mesh/mackesd/src/workers/probe.rs:1-16) but the SHELL never consumes it (grep for probe-inventory/probe/changed in crates/desktop returns nothing); it only appears as host-enrichment service labels inside the Explorer UNIT card (explorer/mod.rs:387,396). (3) VM-internal/KVM service health: event/kvm/services rendered in the Fleet plane (datacenter.rs:48 SERVICES_TOPIC). These live in 3 separate surfaces. The unit_aggregator that DOES union sources (unit_aggregator/sources.rs) merges mesh peers / cloud object …

</details>

### WL-RUN-006 — Router discovery and firewall commit-confirm control
*group:* `crit-run` · *actual:* **partial**

**Remaining:** Build the 'mutations fast-follow' stage: generalize infra/tofu/edgeos/ to per-appliance (per-node tfvars + state/router/<mac>), add an action/router/* firewall-edit verb wrapping Vyatta commit-confirm + typed-confirm + hash-chain audit, and fixture-banner unit tests. Reconcile the mutation panel location (design cites retired mde-workbench -> route via Device Manager/datacenter surface). The final live-router smoke needs a test appliance or a risky mutation of the production farm gateway (operator-gated).

<details><summary>evidence</summary>

READ SLICE done: crates/mesh/mackesd/src/router_discovery.rs (default-route + gateway-MAC discovery + Vyatta-CLI fingerprint) and workers/router_registry.rs (registered unconditionally spawn.rs:2599) resolve sealed router/<mac> cred, probe show version, publish RouterEntry -> mesh/devices/router/<mac> + <host>/router-registry.json, surfaced as Device Manager HostKind::Router. Honest state tested: no_cred_is_unmanaged_needs_creds (:289), cred_present->managed/edgeos (:305), cred_present_but_unreachable->managed/unknown (:314). Firewall COMMIT-CONFIRM auto-revert EXISTS but ONLY for the single hardcoded farm EdgeOS gateway via infra/tofu/edgeos/apply-firewall.sh (var.edgeos_host, single state) + apply-nat.sh/apply-vpn.sh. MISSING: no generalized per-appliance action/router/* firewall/NAT/VPN mutation verb, no per-node tfvars + state/router/<mac> backend generalization, no panel mutation UI …

</details>

### WL-SEC-002 — Federation runtime enforcement and two-mesh accept flow
*group:* `mesh-sec` · *actual:* **partial**

**Remaining:** Runtime enforcement bridge consuming grants; cert install on accept; cross-mesh identity checks at message routing; shell GUI accept/refuse; local two-identity federation harness + integration test proving foreign mesh cannot connect without accept.

<details><summary>evidence</summary>

crates/platform/mde-bus/src/cli/federation.rs is a complete config lifecycle (mint/accept/revoke/rotate) with single-use passcode enforcement (consume_matching_mint:337 fails closed, cmd_accept:387) and audit events (:407 pair-established). BUT: (1) no runtime consumer — grep for subscribe_topics/publish_topics/excluded_topics finds zero readers outside federation.rs; the grants in federation.yaml are inert. (2) accept never installs the cross-mesh nebula trust cert — the /etc/nebula/federation-trusts/<id>.crt path appears ONLY in cmd_revoke (federation.rs:477, delete); header line 6 literally says 'cert install ... — revoke only'. (3) no mackesd worker reads federation.yaml (workers/apps_installed.rs:10 and compute_registry.rs:506 state 'there is no bus-federation worker'). (4) no GUI accept/refuse flow in mde-shell-egui (only a chat/mod.rs comment mentions federation).

</details>

### WL-SEC-004 — Phone remote-input authorization and visible indicator
*group:* `mesh-sec` · *actual:* **mostly-done**

**Remaining:** Add the shell arm/disarm consent publisher to ARM_TOPIC (the one absent link); optional loopback KDC remote-input smoke to prove reject→consent→active→revoke end to end.

<details><summary>evidence</summary>

crates/mesh/mackesd/src/workers/seat_remote_input.rs implements the full consent gate: injection refused unless a live arm grant exists (:435-451), phone-binding check so only the bound phone injects (:457-458), arm TTL clamped to MAX_ARM_TTL_MS=300s (:55), disarm/revocation + per-event audit (:531-576); worker is spawned on Workstation (bin/mackesd/spawn.rs:2409). On-seat indicator is real: worker publishes state/seat/remote-input/<node>; shell status.rs:329 latest_remote_control consumes it and :674-678 renders StatusSegment::RemoteControl as 'Remote control active/armed/disarmed'. Media keys via MPRIS/playerctl in workers/kdc_host/media.rs. THE GAP: a repo-wide grep for the arm topic 'remote-input-arm' finds ONLY the worker (ARM_TOPIC seat_remote_input.rs:29) — NOTHING publishes it. phones_hub.rs (2013 lines) does pairing/QR/roster but has no arm/consent action. So the seat is never a …

</details>

### WL-TEST-001 — OpenStack live and contract tests
*group:* `docs-test-maps` · *actual:* **partial**

**Remaining:** Extend the #[ignore] live test to create a tiny throwaway resource (heat stack or nano server), assert, then delete with guaranteed cleanup even on assertion failure; add the farm lane wrapper + docs. Live execution is gated on a farm OpenStack endpoint that does not yet exist.

<details><summary>evidence</summary>

Contract half DONE: 16 canonical fixtures in crates/mesh/mackesd/src/workers/openstack/client/contract_fixtures/*.json, embedded via include_str! (contract.rs:64-83) and replayed by the offline contract suite (contract.rs, incl. all_fixtures_are_valid_json contract.rs:89). Live half INCOMPLETE: the single live-gated test contract.rs:832 live_openstack_catalog_and_resources (#[ignore], gated on MDE_OPENSTACK_LIVE_TARGET at contract.rs:837) only authenticates + reads catalog/health + LISTS Nova servers (contract.rs:847-877). It does NOT create a throwaway server/resource, does NOT delete it, and has NO cleanup-on-failure — so acceptance 'creates a tiny throwaway server or equivalent harmless resource, and deletes it; cleanup runs on failure' is unmet (grep for create/delete/throwaway/cleanup in contract.rs returns nothing). The client CAN do it — HeatSource::heat_create/heat_delete exist ( …

</details>

### WL-UX-005 — Start Menu and Front Door launcher overhaul epic
*group:* `browser-perf-ux` · *actual:* **mostly-done**

**Remaining:** 1) Parity coverage + removal of the duplicate legacy Start Menu (AC12 / 'no duplicate launcher' outcome). 2) Peer-app actual remote process execution (currently launch request is published; real exec open). 3) Live .15 visual proof of panel/full-screen/keyboard-only modes (seat-gated, not autonomously drainable).

<details><summary>evidence</summary>

The unified Front Door launcher is real, not a mockup (front_door.rs, 5554 lines). Start button (main.rs:1052) and clean Super tap (main.rs:1820 toggle_front_door_panel) both route to open_front_door_panel, which closes Start Menu + opens Front Door (main.rs:1013-1016). Acceptance largely met in code: shared taxonomy dock::LAUNCHER_GROUPS with invariant test 'every Surface::ALL entry must appear...exactly once' (dock/mod.rs:268,330) and Browser+Bookmarks grouped together (dock/mod.rs:292) satisfies AC6; '>' command mode → FrontDoorTarget::RunCommand → console SpawnTab → terminal (main.rs:2474-2477) satisfies AC4; filter chips FrontDoorFilter (front_door.rs:77); mesh-source gating FrontDoorSourceStatus (front_door.rs:58); peer apps FrontDoorPeerApp + LaunchPeerApp (front_door.rs:241,252); favorites reuse the Start pin store; AccessKit + rendered-proof PNGs per evidence log; Front Door has …

</details>

## ❓ Needs operator decision — a named dependency is an unmade decision  (3)

### WL-ARCH-002 — Cloud resource verbs, forms, and typed arming
*group:* `cloud-arch` · *actual:* **mostly-done**

**Gate:** the AC as written ('unsupported verbs are absent, not dead buttons') is already satisfied; confirm whether generic network/volume/image forms are still wanted given the -001 OpenStack-exit direction, and note live-smoke verification is test-project gated

**Remaining:** Only the Required-outcome SPIRIT gap remains: generic create/update/delete forms for network/volume/image (currently list/show only, verbs honestly absent). Live create/delete smoke in a throwaway project is a listed Dependency (OpenStack test project) — GATED. Also unresolved: building MORE OpenStack verbs conflicts with WL-ARCH-001's operator-directed OpenStack exit.

<details><summary>evidence</summary>

AC met at the letter. verbs.rs implements instance lifecycle start/stop/reboot/delete (verbs.rs:112-121, LifecycleAction verbs.rs:170-206) and Heat CRUD heat-check/create/update/delete + get-heat-preview (verbs.rs:119-121), keypair create (verbs.rs:345); destructive ops audited (verb_audits verbs.rs:152, heat_verb_audits verbs.rs:132). iac surface has typed-arming: Arming struct (iac/mod.rs:299) for lifecycle, HeatArming (iac/mod.rs:361) for Heat CRUD; create form fields (iac/mod.rs:402-407); issue_lifecycle publishes real armed Bus verb action/cloud/instance-* (iac/mod.rs:806), issue_heat_mutation (iac/mod.rs:926). NO dead buttons: services with no landed verb render honest caption 'No management verbs are wired for this service yet.' (iac/menubar.rs:217-219), §8 'an absent verb is omitted...never a dead entry' (menubar.rs:9). Resources tab lists/shows all cataloged services via Drill/R …

</details>

### WL-FUNC-005 — Unified search and omnibox indexing
*group:* `browser-func` · *actual:* **partial**

**Gate:** the epic's own dependencies name two unmade decisions — file-indexer storage and search-privacy policy — that gate the headline 'file indexing'; the omnibox/ranking sub-slices can drain in parallel

**Remaining:** Persistent file indexer (blocked on the named 'file indexer storage decision' dependency); live Assistant/AI candidate producer feeding SearchDomain::Assistant; explicit health-weighted mesh ranking; confirm the search privacy policy (also a named dependency). The ranking/assistant-producer slices are code-drainable; the storage + privacy-policy decisions are not.

<details><summary>evidence</summary>

Unified search core spans App/File/Mesh/BrowserBookmark/BrowserHistory/WebSuggestion/Assistant via mde_egui::search_omnibox (front_door.rs:9, filter tabs 144-171); mesh health is folded into search terms (648-649) and a degraded-source status row renders (test 4077); Browser omnibox file:// suggestions are wired (worklist-cited slice, front_door imports mde_files_egui FileSearchTarget:11). BUT: file 'index' is a bounded on-demand HOME snapshot, not a persistent indexer; the Assistant domain has NO live producer — it appears only in tests (front_door.rs:4418), so 'AI-ranked candidates' is unshipped; mesh 'rank by health' is health-as-searchable-term + match tier, not explicit health weighting.

</details>

### WL-UX-003 — Accessibility consumer and application sweep
*group:* `browser-perf-ux` · *actual:* **partial**

**Gate:** does 'consumable tree + annotations + live regions' satisfy the epic, or is a real accesskit_consumer screen-reader + local TTS required (a large build needing a live AT-client seat smoke, and currently governance-deferred)? The annotation sub-sweep (Explorer, Curtain, companion apps, toast widget) IS autonomously drainable, but the epic's defining acceptance (a real consumer path) is a strategy decision + live consumer smoke, so the epic as a whole is not cleanly drainable.

**Remaining:** Decision + build of a real AccessKit consumer + TTS (a11y-02) OR explicit scope-down to seam+tree; add a persisted a11y setting alongside MDE_A11Y; AccessKit annotation sweep for Explorer + Curtain (0 today) + companion apps; live consumer/screen-reader smoke. Annotation sweep is autonomous; consumer/TTS path is decision- and seat-gated.

<details><summary>evidence</summary>

The producer + consumer SEAM is wired and real: A11yBridge::from_env() + enable() in the DRM present loop turns on egui AccessKit tree generation on an MDE_A11Y=1 seat (drm.rs:1237-1238; A11Y_ENV const a11y.rs:35). Live regions exist: status bar Polite + critical-edge Assertive (status.rs:720,756; tests assert Live::Polite/Assertive). Raw-cell annotations landed on Start menu + Browser (per evidence log) and are present in VDI (vdi/mod.rs, 9 accesskit refs), Device Manager (device_manager/mod.rs, 12), Chooser (chooser/render.rs, 14). BUT the Problem statement's headline — 'a real consumer/screen-reader path' — is NOT implemented: the only AccessKitSink impls are BOTH #[cfg(test)] recording stubs (a11y.rs:281 Probe, main.rs:4233 Capture); the default runtime sink is LatestTree which 'merely retains the latest tree' (a11y.rs:57); a11y-02's real screen-reader-over-accesskit_consumer + TTS i …

</details>

## ⛔ Park-blocked — gated on operator / hardware / live-infra / release authority  (16)

### WL-ARCH-001 — Construct Cloud provider-neutral runway and OpenStack exit
*group:* `cloud-arch` · *actual:* **partial**

**Gate:** core (replacement backend, provider-disable proof) is gated on operator provider decision + live cloud test bed + credentials; the AC1 copy-scrub + facade-wiring slice is separately drainable and could be split off

**Remaining:** AC1 (drainable): scrub OpenStack/Nova user-facing copy from cloud_plane.rs, front_door.rs:415/462, console/mod.rs:619; wire the cloud.rs facade into real consumers (iac still on openstack module). AC3/AC4/AC5 (GATED): stand up a replacement cloud provider backend + provider registry/disable toggle, prove it lists+launches a workload over mesh networking, then banner/archive stale OpenStack docs. AC4/AC5 need the replacement-provider DECISION (operator), a farm dev cloud/test bed (infra) and live cloud credentials — all listed Dependencies.

<details><summary>evidence</summary>

Provider-neutral facade EXISTS but is dormant: crates/mesh/mackes-mesh-types/src/cloud.rs:1-232 (CloudProviderAdapter enum + parse_service_catalog_json/parse_resource_table_json accept a non-OpenStack fake, 6 tests), registered mackes-mesh-types/src/lib.rs:22 — yet grep for `mackes_mesh_types::cloud` outside cloud.rs returns ZERO consumers; iac/mod.rs:53 still imports mackes_mesh_types::openstack. iac copy was scrubbed (menubar.rs:718 asserts no 'OpenStack'), BUT a second LIVE cloud surface cloud_plane.rs (mounted workbench.rs:220 Plane::Cloud, rendered main.rs:1209) still shows user-facing OpenStack/Nova: cloud_plane.rs:1684 'OpenStack not configured', :1847 'OpenStack tenant instances (Nova)', :1860 'querying the Nova roster', :1863 'OpenStack not configured'; front_door.rs:415/462 catalog targets 'OpenStack instances/catalog'; console/mod.rs:619 label 'OpenStack Servers'. NO replaceme …

</details>

### WL-BUILD-001 — Immutable bootc, ISO, and RPM release gate
*group:* `build` · *actual:* **blocked-confirmed**

**Gate:** final acceptance needs signing material + release authority + a live physical boot on .15 (operator authorized the .15 wipe/reinstall at WORKLIST.md:37-38, but the signed-ISO cut and physical boot media cannot be crossed by an autonomous session). Static/farm/qcow2 layers already done — do NOT redrive them.

**Remaining:** Signed anaconda-ISO cut + RPM GPG signing (operator /release), bootc image registry publish (operator /release), and the live physical boot-to-egui-seat + role-selection acceptance on .15 (hardware media/boot). virtio-gpu->egui fast path (QC-23) still fallback-only.

<details><summary>evidence</summary>

Image lane is heavily built and the block is real. Built/green: packaging/bootc/build-image.sh (typed base-image gate rc2/3 + optional bootc-image-builder disk lane); packaging/bootc/verify-image.sh:23-95 (static payload+wiring checks: binaries, seat unit + role gate, enablement symlinks, graphical default — pass on the built image); packaging/bootc/README.md:138-151 documents the bootc update/rollback story; README.md:195-213 records a LIVE qcow2 boot proof on BigBoy XCP 2026-07-10 (ovs/libvirt/cloud-init active in-guest); packaging/kickstart/magic-on-quasar.ks:100-163 implements role onboarding %post (role-pin, headless mask, firstboot single-use join); install-helpers/verify-rpm-payload.sh + install-helpers/verify-boot-recovery.sh are the payload/recovery gates. BLOCK is confirmed: grep for gpg|rpmsign|addsign in build-rpm-fedora43.sh/xcp-build.sh returns nothing (no signing code); ma …

</details>

### WL-BUILD-002 — Farm shared cache and fresh-farm bootstrap
*group:* `build` · *actual:* **partial**

**Gate:** acceptance is a live-farm proof loop (cross-node hits + fresh bootstrap + slot drain) needing live farm VMs, a minio backend, and sealed credential material. Operator authorized the sccache 'first slice' (WORKLIST.md:44) and the tooling is ready, so this is executable by a live farm-orchestrating session with control-host root once the backend+sealed creds exist — it is not a hard block, but it is not autonomously drainable under the live-infra/passphrase criterion.

**Remaining:** Stand up the control-host minio/S3 backend, seal sccache-access-key/sccache-secret-key, run sccache.yml against build_vms (or re-bake+roll the golden image), then prove Node-A->Node-B cache hits + a fresh-farm bootstrap one-shot + clean slot drain.

<details><summary>evidence</summary>

Tooling is complete but NOTHING is live. Present: install-helpers/farm-sccache-proof.sh:30-101 (per-node contract probe); infra/ansible/sccache.yml:15-31 (installs sccache 0.8.2, S3/minio backend); install-helpers/bake-build-golden.sh:2,26,94-97 (golden bake runs sccache.yml); automation/farm/farm-bootstrap.sh:1-12 (DAR-36 one-shot bring-up ordering state->sccache->golden->tofu); install-helpers/farm-vm-snapshot.sh + farm-slot-gc.sh (snapshot/revert + slot cleanup). NOT LIVE: WORKLIST.md:334-339 records the 2026-07-17 proof reaching .50/.90/.130/.170 with all four reporting no sccache binary and no ~/.sccache.env; farm-sccache-proof.sh:90-95 would therefore exit 1. Gate: infra/ansible/sccache.yml:4-13,26-35 requires a minio endpoint plus sccache-access-key/sccache-secret-key sealed in the mesh secret store (mcnf-secret.sh), with 'no baked LAN default' — the old .192-built minio is explic …

</details>

### WL-CRIT-001 — Mesh VDI console broker end to end
*group:* `vdi-media` · *actual:* **mostly-done**

**Gate:** code + unit tests complete; the acceptance's live two-node VM console proof needs a libvirt guest with a live console plus two overlay-reachable nodes running socat/virsh — hardware/live-infra gate

**Remaining:** Only the live end-to-end proof: broker a real local libvirt VM's SPICE/VNC console on one peer and connect from another peer with frame+input round-trip and frame-checksum/video evidence. The socat relay + virsh path has never been exercised live.

<details><summary>evidence</summary>

console_broker.rs:94-283 resolves live console (parse_domdisplay from `virsh domdisplay`), builds socat overlay relay args (build_relay_args), and publishes typed ConsoleStatus::Brokered/Unbrokerable to CONSOLE_TOPIC="state/vdi/console"; spawned at bin/mackesd/spawn.rs:1356-1364 and census-registered as a universal rank-0 worker at worker_role.rs:930; shell consumes it at vdi/mod.rs:178 (resolve_brokered_console) + :1323 (poll_brokered_endpoint) which holds an honest 'Resolving...' state (:1264-1270) then spawns the live transport once the endpoint lands; 21 unit tests in console_broker.rs (resolve/relay/serving-peer/full-pipeline).

</details>

### WL-CRIT-004 — Control-plane DR backup and guided rebirth
*group:* `crit-run` · *actual:* **mostly-done**

**Remaining:** Only the live DR drill remains: the acceptance items 'a fresh node can enroll after restore' + 'leader election is healthy' need a rebirthed fresh control node (live infra), and the off-fleet CA bundle push needs the operator-held MDE_BACKUP_PASSPHRASE + DO Spaces keys. Per the 2026-07-16 decision an agent may now run dr-backup export + on-mesh snapshot + dr-reconstitute --verify (dry-run) locally, but the rebirth-and-enroll proof is operator/live-gated.

<details><summary>evidence</summary>

Full DR toolchain present + wired: automation/dr/{dr-backup,dr-restore,dr-reconstitute,dr-ca-bundle,dr-push-offfleet,dr-snapshot-onmesh,dr-verify-offfleet}.sh. dr-reconstitute.sh does content-verified guided rebirth (admin row + named seed repo assert, lines 133-152). Scheduler worker crates/mesh/mackesd/src/workers/dr_scheduler.rs registered at worker_role.rs:560 + spawn.rs:1907, leader-gated, bounded, 9 unit tests. Confirm-gated RPC 'dr-backup' at ipc/host_ops.rs:70,664-706. Passphrase read from MDE_BACKUP_PASSPHRASE->tmpfs->--passphrase-stdin/--passphrase-file, never argv (dr-ca-bundle.sh:60-97). Runbook docs/help/mesh-recovery.md (Case A/B rebuild).

</details>

### WL-FUNC-001 — Browser protected media and hardware media path
*group:* `browser-func` · *actual:* **mostly-done**

**Gate:** Widevine bundle + DRM account + live seat = operator/legal/hardware gate; the media-keys/PiP/media-session slices and the named-requirement gate are already done — only the real-DRM 'passes' outcome is blocked

**Remaining:** Wire the CDM into the engine load path (untestable without a bundle); operator legally fetches a Widevine bundle to /opt/mde/widevine; live DRM/Netflix-equivalent smoke on a seat with a DRM test account. GPU/HW decode is CEF-OSR-hard and not in the acceptance line.

<details><summary>evidence</summary>

Widevine is DETECTED/validated/sandbox-bound only — mde-web-cef/src/lib.rs:438 detect_widevine + status_line (194/286), renderer.rs:454 validate_widevine + 359 sandbox bind — but NO CDM is ever registered with the engine: `--widevine-cdm-path`/RegisterWidevineCdm appear NOWHERE (grep of cef_init.rs command_line_switches:399, renderer, cef_browser all empty), so even a real bundle would not load. Media keys ARE fully wired: web/mpris.rs owns org.mpris.MediaPlayer2.mde-browser (interfaces 293/411, Play/Pause/Next), spawned at startup (main.rs:980). PiP is a shell-owned overlay with a real model+toggle (web/mod.rs:2680 browser_media_pip_model, MenuAction::TogglePictureInPicture, tab_should_be_hidden PiP logic 9580). Normal browsing works without CDM (WidevineCdm::Missing launches). GPU/HW decode ABSENT (no vaapi switch in cef_init.rs). Only widevine detection UNIT tests exist (lib.rs:1322)  …

</details>

### WL-FUNC-002 — Browser passkeys, hardware keys, and phone authenticator
*group:* `browser-func` · *actual:* **partial**

**Gate:** headline 'hardware key login works' needs a physical FIDO2 key + test IdP = hardware gate; software passkey + consent + honest UP/UV are done and could be split off as mark-done

**Remaining:** Implement CTAP2 CBOR credential commands (makeCredential/getAssertion) over /dev/hidraw for real hardware-key login; build phone-as-authenticator (KDC); live WebAuthn proof against a controlled relying party.

<details><summary>evidence</summary>

Software platform authenticator is COMPLETE: mde-browser-workers/src/browser_passkeys.rs mints P-256 creds, builds the WebAuthn attestation object (cbor fmt:none 1700-1725), and the module header (lines 11-21) plus code keep UP honest (set only from PasskeyRequest::user_present) and UV ALWAYS unset — satisfying 'UV never asserted without real verification'. Shell consent prompt is wired (web/mod.rs:629 PendingPasskeyConsent, 5640 gate). Hardware path is a READINESS PROBE + CTAPHID_INIT diagnostic ONLY (browser_passkeys.rs:74-98 framing, 216-232 live-probe state, env-gated MDE_BROWSER_PASSKEY_CTAPHID_LIVE_PROBE) — NO CTAP2 makeCredential/getAssertion CBOR over hidraw. Phone-as-authenticator is ABSENT: header lines 8-9 explicitly 'CTAP2 credential commands, phone-as-authenticator, and live relying-party E2E proof remain separate owners.'

</details>

### WL-FUNC-007 — Media local video and library/art browse proof
*group:* `vdi-media` · *actual:* **mostly-done**

**Gate:** acceptance is a live seat smoke needing libmpv + a media library source; the render-agnostic code + FakeMpv unit tests are done, but frame-render + service-backed art cannot be proven autonomously

**Remaining:** Live seat proof that real mpv frames advance on the Media stage (needs an F44 seat with system libmpv and the media-mpv build), controls work live, and library/artwork browse against a live configured media service; embedded-tag/artwork enrichment against a real source is only partially present.

<details><summary>evidence</summary>

Real libmpv engine mde-media-core/src/mpv.rs:340-376 (latest_frame → RGBA VideoFrame via mpv screenshot at ~150ms), feature-gated `mpv` (Cargo.toml:75) with FakeMpv default; shell/app upload path mde-media-egui/src/app.rs:1592-1660 (VideoTextureCache pulls latest_frame and set()s a texture); library.rs is a real std::fs recursive walk (index_folder:356, browse/search folds) — unit-tested vs FakeMpv. Artwork enrichment (embedded tags/TMDB) is honestly gated (library.rs metadata doc: only fs-derived title+kind without a tag/probe dependency).

</details>

### WL-FUNC-009 — Sunshine/Moonlight shadowing of the Magic Mesh shell
*group:* `vdi-media` · *actual:* **partial**

**Gate:** the policy/plan/helper/firewall/systemd/indicator scaffolding is complete, but the remaining interactive pairing-approval bridge and every frame/input/disconnect/exposure acceptance require a live DRM Workstation seat with a hardware encoder, a running Sunshine service, and a real Moonlight client — hardware/live gate

**Remaining:** Native interactive pairing: intercept a real Sunshine/Moonlight pairing request into a shell modal naming the client and gate acceptance on local approval; plus live proof of Moonlight advancing frames, hardware-encoder use, remote input round-trip, disconnect revocation, and exposure-switch reachability (mesh-only + lan).

<details><summary>evidence</summary>

Declarative/infra half landed: Settings 'Remote Proofing' policy + effective service plan (system/mesh.rs:413-524 config toggles native_pairing_prompt/require_local_approval/show_shadowing_indicator/allow_remote_input/vnc_fallback/min_fps + capture/encoder + bind scope; system/mod.rs:2015 config; tests.rs:1128-1182); packaged helper install-helpers/mde-remote-proofing-apply.py (40KB) renders plan/lifecycle/sunshine.conf, models mesh/LAN/public firewalld intent, resolves LAN bind from ip -j route/addr, reconciles only owned rich rules, and supervises the user sunshine.service via runuser; Workstation-gated systemd path/service watcher (packaging test full_rpm_ships_remote_proofing_bridge_but_server_variant_does_not); shell status rail 'Remote control' indicator wired to the seat remote-input retained record; seat_remote_input.rs provides the armed/consent remote-input seam. The 'Native sh …

</details>

### WL-FUNC-010 — Native Maps & Location workspace and offline navigation readiness
*group:* `docs-test-maps` · *actual:* **mostly-done**

**Gate:** the entire hardware-independent scope (simulator, readiness guardrails, manual-switch, offline-map/dead-zone, MG90 setup model, tessellation, stable shell mount) is COMPLETE and farm-verified and satisfies every non-live acceptance criterion. The only remaining work is the real hardware/daemon adapters, which the epic itself scopes as 'later' and its Dependencies gate on hardware (MG90/gpsd/routing+geocoder daemons/CAN fixtures). Move it Remaining→Blocked(hardware); the current simulator deliverable can be marked done.

**Remaining:** Real adapter implementations behind the existing typed seams: MG90 mgmt/firmware, gpsd, Valhalla routing, Nominatim geocoder, CAN/GPIO, serial recovery, encrypted local vault, plus live MG90/gpsd/map/routing proof — all hardware/daemon-gated, not drainable autonomously.

<details><summary>evidence</summary>

crates/desktop/mde-maps-location-egui/ is real, substantive, and shell-mounted, not a mockup. Guardrail model: OfflineNavigationStatus::from_surface (model.rs:297) pushes hard blockers for every acceptance-1 case — no/unhealthy primary source (model.rs:306,326), stale primary (:312), no loaded offline map (:352), storage over cap (:355), routing/geocoder provider contracts unavailable (:368-378), MG90 setup not offline-maps-verified (:381), MG90 unauthenticated (:394); can_claim_turn_by_turn returns false when Blocked (model.rs:442). Manual-switch (acceptance 2): LocationSourceManager.auto_failover=false (model.rs:1301-1311) with healthy_alternatives() offering manual switches only (model.rs:1362), blocker text 'manual switch required because automatic failover is off' (model.rs:337); tests primary_source_never_auto_failovers (model.rs:2048) + manual_switch_readiness_requires_connected_f …

</details>

### WL-RUN-003 — Lighthouse full/equal join and push-button add/retire
*group:* `mesh-sec` · *actual:* **mostly-done**

**Gate:** the typed add/retire, etcd voter membership, CA custody/inheritance, and quorum-preserving drain-gate are all implemented in code. The open acceptance is the live add-retire-add cycle drill on a live multi-lighthouse fleet with etcd health proof, and `lighthouse add` provisions a real DO droplet (needs DO API creds/context) — operator/live-infra gated, not autonomously drainable.

**Remaining:** Live add-retire-add cycle on the live fleet: add a new lighthouse (CA/enroll + etcd voter status confirmed), retire one preserving quorum, re-add; capture etcd health proof. Requires live multi-LH fleet + DO provisioning credentials.

<details><summary>evidence</summary>

crates/mesh/mackesd/src/cli/node_admin.rs:164 lighthouse_add mints a role-scoped Lighthouse token and shells do-lighthouse-join (present at install-helpers/do-lighthouse-join.sh) to stand up a droplet that joins as a FULL lighthouse (CA signer + etcd voter, am_lighthouse); :205 lighthouse_retire runs drain_gate then remove-peer (decommission+revoke+ban+etcd member-remove) then deletes the droplet last. cli/join.rs:229-273 add_self_as_voter_blocking auto-joins the etcd quorum (no manual etcdctl); :197-206 inherits the mesh CA (MIG-3) and provisions the CA-backup passphrase. lighthouse_lifecycle.rs:17 drain_gate holds HA_MIN_LIGHTHOUSES floor so retire preserves quorum; cli/leave.rs:43 removes self so no ghost voter. spawn_lighthouse_onboard worker + shell spawn_lighthouse_flow provide the operator UI.

</details>

### WL-RUN-004 — Media lighthouse production service, failover, and upload path
*group:* `vdi-media` · *actual:* **blocked-confirmed**

**Gate:** dependencies are explicitly a live DO Spaces bucket/keys and two live Lighthouse_Media nodes; verification is a live media drill — operator/live-infra gate

**Remaining:** Live 2-node acceptance: two Lighthouse_Media nodes serving one library over DO Spaces, music.mesh DNS failover on kill-one, non-admin account provisioning, upload→rescan visibility, and fresh-Workstation auto browse/play — all against live nodes with real DO Spaces keys.

<details><summary>evidence</summary>

Real code slices exist: media_registry.rs:212-220 probes navidrome health and publishes registration_with_account pinned to music.mesh:4533; media_server.rs:906-976 rescans shared folders into a manifest and republishes on a 30s cadence; music_autoconfig.rs reads the published shared account and writes the Workstation creds JSON; navidrome_supervisor.rs/media_navidrome.rs/media_sources.rs present; automation/media/{ingest-music.sh,verify-media-lighthouse.sh}.

</details>

### WL-SEC-001 — Fresh-node enrollment bootstrap and final join path
*group:* `mesh-sec` · *actual:* **mostly-done**

**Gate:** code complete (token/endpoint/join/boot-durable all implemented, DAR-19 fixed); the only open acceptance is the live fresh-node join drill on .15 (obtain overlay IP + survive reboot), a destructive wipe of a physical/VM node against a live lighthouse. Operator pre-authorized .15 (2026-07-16 decision), so it is live-drill-ready but not autonomously drainable.

**Remaining:** Live fresh-node join drill on .15: join from the advertised token with no manual endpoint correction, confirm overlay IP, reboot and confirm re-convergence. Parser/unit tests for legacy+v3 forms already present.

<details><summary>evidence</summary>

crates/mesh/mackesd/src/bin/mackesd.rs:3250 resolve_enroll_endpoint_host never falls back to overlay IP and re-derives DEFAULT_ENROLL_PORT (4243); :3371 invite_issue_join_token_from_cert builds v3 token with public host + enroll port + cert fp; cli/join.rs:106-162 handles all 3 token forms (MDEINV1 invite endpoint-gated, v3+fp network enroll, legacy no-fp co-located); nebula_enroll_client.rs:1-10 is a real fingerprint-pinned CSR-over-HTTPS enroll client (not a stub); cli/join.rs:168,218-225 enables nebula+mackesd+mesh-health so a joined node survives reboot. DAR-19 regression test at mackesd.rs:3300-3322 asserts never-overlay/never-4242.

</details>

### WL-SEC-003 — Secret-store distribution and scoped decryption roots
*group:* `mesh-sec` · *actual:* **partial**

**Gate:** 'two authorized nodes decrypting the existing sealed DO token'), needing a live second node + operator-owned DO creds; that is not autonomously drainable. Drainable slice exists though: add role/scope-targeted sealing (per-role recipient subsets for datacenter/media/router/DR/control) + offline fixture tests for recipient selection and rotation.

**Remaining:** (a) Role-scoped recipient sealing so a secret replicates only to authorized roles, not the full set — DRAINABLE, plus offline fixture tests. (b) Live two-node decrypt proof of the existing sealed DO token + unauthorized-node rejection + rotation redistribution — BLOCKED on live second node and operator provider creds.

<details><summary>evidence</summary>

automation/secrets/mcnf-secret.sh is a working multi-recipient age store on etcd: init/init-self (per-node identity+recipient registration :192-211), put (:214-226 seals to full recipient_set — age multi-recipient so two authorized decrypt, unauthorized cannot), get (:229-241 decrypt with local key), plus rotate/reseal-to/reseal-all/recipients; secrets pass via stdin (no argv leak) and only public keys are printed; a MOCK_DIR harness (:71) exercises reseal offline. GAP: put ALWAYS seals to the FULL recipient_set (:156-171, :217-226) — every registered node reads every secret, i.e. the exact fleet-wide-decryption-root concern the epic raises; grep for scope/role/subset targeting in the script returns nothing. openstack/secrets.rs:287 load_or_seal is a separate leader-minted OpenStack-secret path, not role-scoped recipients.

</details>

### WL-TEST-002 — Crown-jewel integration harness for real etcd/Nebula
*group:* `docs-test-maps` · *actual:* **mostly-done**

**Gate:** the harness code is essentially complete; the epic's defining acceptance is a live full farm run needing root podman + image egress on the airgapped farm + farm VM capacity + approved destructive boundaries (its stated dependencies). A thin farm-runner wrapper + artifact capture is the only autonomously-draftable slice; the run itself is operator/infra-gated.

**Remaining:** Optionally author a farm-runner wrapper that invokes `cargo test --features docker-tests` on a root-podman farm host and captures logs/artifacts on failure; then perform the one gated full run (needs airgapped image availability for fedora:42 + quay.io/coreos/etcd, farm capacity, and approved destructive boundaries).

<details><summary>evidence</summary>

The harness CODE exists and is comprehensive. Real-etcd election: crates/mesh/mackesd/tests/substrate_etcd.rs spins a real single-node etcd container via sudo podman and drives election — etcd_leader_election_elects_one_renews_and_force_takes (substrate_etcd.rs:99, force-take at :157). Real-Nebula overlay+enroll: crates/mesh/mackesd/tests/integration_testcontainers.rs spins real container nodes (image built from install-helpers/nebula-test-node.Containerfile) running freshly-built mackesd + real nebula, driving mesh-init/serve/enroll and asserting bidirectional overlay ping (file header + force_take epoch bump at :449-466). Both are #![cfg(feature="docker-tests")] (Cargo.toml:339), self-skipping when root podman/tun absent, and tear down their containers. Recovery is also covered on the farm testbed by automation/testbed/test-stability.sh (soak/chaos/reboot via farm-testbed.sh nodes .60/ …

</details>

### WL-UX-001 — Win10 hybrid bottom taskbar and tray live proof
*group:* `build` · *actual:* **blocked-confirmed**

**Gate:** the sole remaining item is a live DRM-seat screenshot/pixel proof on .15 (a live-hardware seat currently carrying the browser work, plus an operator/live-eye visual sign-off). All geometry/overlap/tray-reachability logic is implemented and test-covered — do NOT redrive the code; only the live visual acceptance is outstanding.

**Remaining:** Deploy the shell to a live DRM seat (.15) and capture screenshots/pixel proof of the bottom taskbar + start grid + tray + show-desktop nub + action center at supported resolutions (the live-eye pass). No code work outstanding.

<details><summary>evidence</summary>

Geometry/tray/start code is code-complete and unit-covered; only live visual proof remains. crates/desktop/mde-shell-egui/src/dock/mod.rs is 2696 lines; dock/tests.rs holds 42 #[test]s covering exactly the acceptance criteria: dock/tests.rs:640 win10_hybrid_31_the_new_tray_cells_do_not_overlap_the_sessions_run (overlap), :468 win10_the_taskbar_is_a_fixed_48px_height_across_densities, :1011 taskbar anchors to true bottom edge, :1063 status panel opens above the rail (not screen top), :900 tray_overflow_flyout_routes_status_segments, :585/:611 action-center + show-desktop-nub routing, :1450/:1689/:1821 AccessKit button/landmark reachability of tray + session cells. start_menu.rs is 5004 lines with persisted pins. BLOCK confirmed by the epic's own evidence WORKLIST.md:1638-1639 ('Live tray/screenshot proof remains the blocking tail') and operator directive WORKLIST.md:73 ('pass/fail is scre …

</details>
