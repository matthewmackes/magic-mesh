# OPEN-vs-DONE Ledger — reconciled 2026-07-11

Scope: every P2/P3 finding in PLATFORM-REVIEW-2026-07-10.md whose primary file is
NOT under crates/mesh/mackesd/ (mackesd-owned findings marked DEFER-arch7 without
deep-check). Verified against HEAD 1e815085 (~100 commits landed after the review).

**Summary: 18 open / 5 partial / 29 done / 5 deferred (57 findings).**
The post-review wave (commits dated 2026-07-11, above review commit e0e8295) fixed
the majority of in-scope findings; the OPEN set is dominated by the a11y cluster
(low-pri, TTS path dropped by operator) and brand-string / doc-hygiene work.

| id (report line) | primary file | status | current loc | evidence |
|---|---|---|---|---|
| arch-2 (301) | mde-shell-egui/src/discovery.rs | OPEN | discovery.rs:45, session_rail.rs:37 | SessionRequest still defined 3× (broker + 2 shell mirrors); not folded into mackes-mesh-types |
| arch-11 (401) | mde-shell-egui/src/ | OPEN | 91 Persist::open sites across src/ | no BusReader/BusClient seam exists; perf-3 fast-path (19737698) cut per-open cost but shared seam absent |
| test-obs-7 (518) | .github/workflows/ci.yml | OPEN | repo root: no .config/nextest.toml | serial-only constraint still docs-only; no nextest override / serial_test landed |
| arch-10 (543) | mde-shell-egui/src/cloud_plane.rs | OPEN | cloud_plane.rs:1058 state_handle / :74 STATE_KEY | CloudPlaneState still parked in egui temp data store; not hoisted to Shell field |
| perf-12 (552) | workspace-wide (measurement) | OPEN | no clippy::unwrap_used in mde-vdi-*/mde-bus | scoped panic-lint recommendation not applied (informational finding, low value) |
| docs-consistency-10 (561) | docs/WORKLIST.md | OPEN | WORKLIST.md still 1,491,176 bytes | no docs/worklist-archive/ split; no line-length lint |
| shell-ux-4 (581) | mde-shell-egui/src/dock.rs | OPEN | dock.rs (0 on_hover_text; lock #11 at :24-25) | no hover-flyout label added; icon-only-no-tooltip lock intact |
| a11y-04 (655) | mde-shell-egui/src/dock.rs | OPEN (a11y low-pri) | pick_app_cell_with_badge dock.rs:2399, system_quad :3585 | neither calls install_cell_accessibility; picker+quad still unannotated |
| a11y-06 (673) | mde-egui/src/toast.rs | OPEN (a11y low-pri) | toast.rs (0 accesskit in 1047 lines) | no Role::Alert / Live region on ToastHost |
| a11y-07 (682) | mde-shell-egui/src/system.rs | OPEN (a11y low-pri) | AppearanceConfig ~system.rs:1251 | no reduce_motion setting; still accent+text-scale only |
| a11y-08 (691) | mde-shell-egui/Cargo.toml | OPEN (a11y low-pri) | companion crates' Cargo.toml | accesskit feature still enabled only by mde-shell-egui; term/media/editor/files featureless |
| shell-ux-2 (746) | mde-shell-egui/src/dock.rs | OPEN | dock.rs:295 GROUPS vs start_menu.rs:420 TILE_GROUPS | two divergent taxonomies persist; no Surface::group() single table |
| docs-consistency-2 (764) | mde-kdc-host/src/fanout.rs | OPEN (brand-inflight) | fanout.rs:35 "Quasar Mesh"; AI_GOVERNANCE.md:12 | superseded s-spelling still shipped; governance doc still says MCNF 12.0 "Quasar" |
| shell-ux-9 (807) | mde-theme/src/brand/logo.rs (consumers) | OPEN (brand-inflight) | phones_hub.rs:58, device_manager.rs:1433 | "Quasar Mesh"/"Magic-Mesh Quasar" user strings not routed through brand consts |
| shell-ux-8 (940) | mde-shell-egui/src/main.rs | OPEN | main.rs show_mesh_map ~:517 | Explorer still not a Surface enum member; mounted only as Mesh-Map lens toggle |
| perf-8 (949) | mde-shell-egui/src/web/mod.rs | OPEN | web/mod.rs:2963 tab.last_frame = Some(img.clone()) | full ColorImage still deep-cloned per frame; no Arc<ColorImage> share |
| arch-13 (958) | crates/platform/mde-lighthouse-health/ | DONE | renamed | d13bd93f: mde-cosmic-applet → mde-lighthouse-health, last cosmic-era crate name retired (lock regen'd) |
| a11y-02 (1013) | mde-egui/src/drm.rs | OPEN (a11y low-pri, TTS dropped) | a11y.rs consumer seam exists; no screen-reader/TTS | AccessKit tree now generated (a11y-01) but no in-process screen reader; operator dropped TTS path |
| browser-3 (135) | mde-web-cef/src/cef_browser.rs | PARTIAL | cef_browser.rs ~2760-2773 | WebRTC block hardened (60a2d0fb) + residual documented (browser-5); airtight all-frame/permission-handler blocked by prebuilt-CEF ABI |
| perf-7 (428) | mde-vdi-spice/src/pixel.rs; vdi.rs | PARTIAL | vdi.rs handle.set; mde-vdi-core (no ImageDelta) | idle-frame emission now gated by dirty-check (eff6dad2), but changed frames still full-frame upload; no partial sub-rect ImageDelta |
| shell-ux-7 (509) | mde-shell-egui/src/web/mod.rs | PARTIAL | web/mod.rs still 19,060 lines | converted to web/ dir + 7 submodules extracted (889b6fc7 +6); core mod.rs still a monolith |
| shell-ux-6 (646) | mde-egui/src/drm.rs | PARTIAL (a11y low-pri) | drm.rs a11y seam added; system/storage/iac = 0 accesskit | AT consumer seam (a11y.rs) + 4 surfaces annotated (a11y-05), but system/storage/iac still unannotated + no shipped AT output |
| a11y-05 (664) | mde-shell-egui/src/explorer.rs | PARTIAL (a11y low-pri) | curtain.rs = 0 accesskit | explorer/device_manager/chooser/vdi annotated (94020327,10059a11,b086406d,2439f1cc); lock-curtain (highest-pri unlock path) still unannotated |
| vdi-vm-6 (171) | mde-vdi-rdp/src/connect.rs | DONE | TOFU cert pinning module | 2fb08ff6 + 0c91b67b: RDP TLS server-key pinning / TOFU replaces blanket no-verify |
| security-4 (180) | mde-web-cef/src/cef_init.rs | DONE | cef_init.rs:141-151 remote_debugging_port() | 60a2d0fb: CDP port now opt-in via env, defaults 0=disabled |
| browser-4 (144) | mde-web-cef/src/cef_browser.rs | DONE | cef_browser.rs:2910/2915 | 60a2d0fb: shim gates on options.publicKey, calls originals for password/federated/otp |
| build-deploy-6 (231) | automation/forgejo/dnf-channel-up.sh | DONE | — | 8b350721: stop indexing unsigned HOLD/ RPMs into client channel |
| build-deploy-7 (239) | install-helpers/build-rpm-fedora43.sh | DONE | — | 760272a5: pin the release cut for hermeticity |
| arch-4 (310) | install-helpers/lint-layered-tiers.sh | DONE | — | 4110b6e6: close layered-tiers GUI-harness loophole |
| arch-12 (328) | mde-shell-egui/src/dock.rs | DONE | verify-rpm-payload.sh | bcb91e07: static gate for "compiles ≠ ships" + dead surfaces |
| perf-5 (410) | mde-shell-egui/src/chat.rs | DONE | — | f48df11b: incremental per-topic ULID-cursor conversation refresh |
| perf-6 (419) | mde-web-cef/src/cef_browser.rs | DONE | — | be388f4e: stop 125Hz idle spin + timer shim re-injection |
| arch-8 (491) | mde-vdi-{rdp,vnc,spice}/src/ | DONE | mde-vdi-core crate | d4fefebe: shared mde-vdi-core extracted from the three transport crates |
| arch-9 (500) | mde-web-preview/src/lib.rs | DONE | mde-web-wire crate | 9401c9b1 + e0244a10: shared browser wire protocol extracted, #[path] includes removed |
| build-deploy-9 (527) | automation/queue/farm-pool-manager.sh | DONE | — | d70d4ff8: install agent unit from current source, not stale ~/magic-mesh |
| test-obs-11 (535) | mde-vdi-vnc/tests/, mde-vdi-rdp/tests/ | DONE | — | 9bd16291: CI-runnable loopback protocol tests for VNC + RDP |
| shell-ux-5 (590) | mde-shell-egui/src/datacenter.rs | DONE | — | 5f5e9b13: two-step confirm on VM stop |
| vdi-vm-8 (599) | mde-shell-egui/src/vdi.rs | DONE | — | 9768754e: negotiate desktop at panel size + re-dial on resize |
| docs-consistency-8 (608) | mde-shell-egui/src/datacenter.rs | DONE | — | 3b23225b: clarify Fleet-KVM vs Cloud-instance lenses (docs) |
| a11y-09 (700) | mde-shell-egui/src/curtain.rs | DONE (a11y low-pri) | — | 61a6566e: enlarge mute-button hit target to comfortable minimum |
| docs-consistency-3 (773) | docs/help/install.md | DONE | — | 720a9b48 + 030004ef + 01e8aa21: retire stale Workbench-Help-panel claim, refresh help |
| docs-consistency-7 (782) | CHANGELOG.md | DONE | — | bb6a5be8: bring [Unreleased] current with 12.0.x wave |
| browser-5 (791) | mde-web-preview/src/engine.rs | DONE | — | cac4c0e3: cross-engine WebRTC/passkey posture documented (finding's doc option); CEF hardening under browser-3 |
| build-deploy-8 (799) | packaging/kickstart/magic-on-quasar.ks | DONE | — | b3feb2b3: reconcile ISO SELinux posture with shipped QC-22 enforcing stack |
| shell-ux-11 (816) | mde-shell-egui/src/chat.rs | DONE | — | 5f5e9b13: keep Chat drafts + surface inline error on Bus-publish failure (verifier note at report line 822) |
| docs-consistency-5 (825) | docs/NEEDS-OPERATOR.md | DONE | NEEDS-OPERATOR.md:61-66 | ac470d20: OW-8 rewritten to Nova/Heat + Glance golden; cloud-hypervisor removed |
| docs-consistency-6 (834) | docs/design/planes.md | DONE | — | fea8a3ed: reconcile planes.md with shipping five-plane + dock nav |
| security-8 (843) | docs/THREAT_MODEL.md | DONE | THREAT_MODEL.md §1 :38, :474-475 | trust table now lists mde-web-cef; passkey flags reconciled to 0x01/0x41 |
| a11y-10 (967) | mde-egui/src/style.rs | DONE (a11y low-pri) | — | a81a4105: WCAG contrast guard test for pressed accent text |
| perf-9 (976) | mde-shell-egui/src/web/mod.rs | DONE | — | 1e815085: gate per-frame session-snapshot rebuild to 1s cadence |
| perf-11 (985) | mde-shell-egui/src/main.rs | DONE | — | 5f5e9b13: stagger Workbench plane polls |
| docs-consistency-9 (994) | docs/BUILD-ENVIRONMENT.md | DONE | — | aa1a3d99: route heavy-build guidance to the farm; drop mde-workbench exemplar |
| arch-7 (319) | mde-mackesd/src/workers/browser_*.rs | DEFER-arch7 | (mackesd-owned) | in-progress this session — concurrent mackesd refactor |
| perf-10 (437) | mde-mackesd/src/workers/kvm_health.rs | DEFER-arch7 | (mackesd-owned) | fix commits landed (b740c94f + ~7 perf-10 in-process-publish commits) but mackesd-owned — defer |
| build-deploy-11 (851) | mde-mackesd/Cargo.toml | DEFER-arch7 | (mackesd-owned) | fix landed (5846dcf6/7415e9ae drop cosmic RPM assets) but Cargo.toml under mackesd — defer |
| build-deploy-10 (1003) | mde-mackesd/Cargo.toml | DEFER-arch7 | (mackesd-owned) | no NEVRA-gate commit found; mackesd-owned — defer |
| build-deploy-12 (1022) | mde-mackesd/Cargo.toml | DEFER-arch7 | (mackesd-owned) | fix landed (6b1ccbab RPM 100MB-ceiling guard) but Cargo.toml under mackesd — defer |

## Dispatch notes (coordinator, 2026-07-11)
- shell-ux-4 (581) + shell-ux-2 (746): superseded/partially-closed by dock commit `4ab13744` (on_hover_text tooltips on app cells + Voice group-drift reconciled). Residual for 746 = single-table derivation (deferred, big refactor).
- brand-inflight (764, 807): OPERATOR-GATED — the codename spelling is NAMING-1, and repo lock #9 says "Quazar" (Z) is canonical. Do NOT sweep either direction autonomously.
- DEFER-arch7: leave for the arch-7 mackesd refactor to land first; three already have fix commits in HEAD.
