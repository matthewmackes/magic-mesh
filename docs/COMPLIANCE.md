# Magic Mesh — Compliance & Integrity Sweep

**Date:** 2026-06-11 · **Scope:** ~21 crates · **Rulebook:** `AI_GOVERNANCE.md` (E11 "Magic Mesh" pivot) · **Lens:** *fit for purpose* — does the platform deliver "a secure, no-fixed-center workgroup mesh and its desktop"?

Verdicts are binary: **FINISH** (make it real / wire it / fix the doc) or **REMOVE** (delete the dead surface). Report-only — nothing was modified. Five parallel sub-audits (mesh/daemon · workbench · services · conventions+docs · platform+packaging); highest-impact findings spot-verified by the synthesizer.

> Supersedes the 2026-06-09 sweep. **Most of that sweep's headline items are now resolved:** the live labwc/sway compositor-driver workers + `window_manager` panel are gone, the Headscale/Tailscale testcontainers were retargeted to a real Nebula overlay E2E (OBS-1), the fleet-revision control plane is implemented + reachable (`fleet_reconcile` shells `magic-fleet reconcile`), and most `mde <subcommand>`-dispatcher doc-drift was fixed. The items below are the *current* state.

## Headline

The mesh **control plane** is genuinely solid and fit for purpose: enrollment, Nebula config materialization, LizardFS replication, the peer directory, fleet reconcile, SIP voice (real REGISTER/INVITE/RTP), and Airsonic/MPRIS music all work end-to-end and are reachable from a real entrypoint. The §7 "shipped-but-dead" discipline is strong — **zero `todo!()`/`unimplemented!()`**, no fake-data-on-a-real-path mockups, and honest empty states where a producer is absent.

The gaps cluster on **the mesh's *data plane* and *distribution*** — the parts that move bytes and ship the product:

1. **File transfer across the mesh is a facade** — "Send to peer" is the SVC-5 headline, and it is a no-op at every seam (#1). The single biggest fit-for-purpose hole.
2. **The phone hub never acts outbound** — KDC ring/send-file enqueue but nothing drains the queue (#2, documented).
3. **The product can't fully ship as intended** — the cosmic-applet (the §5 Cosmic integration) is packaged nowhere, the install-time role chooser has no GUI, and the DISCLAIMER pre-flight accept gate is build-time-only (#4–#6).
4. **Two convention lint-gates `AI_GOVERNANCE.md` claims to enforce don't exist** (only the §6 boundary lint runs) — which is why a §2 private D-Bus name and a §4 parallel token module slipped in (#8–#10, #21).

## Findings — fit-for-purpose gaps (capabilities claimed but not wired end-to-end)

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **1** | `mde-files/src/app.rs:653` (`SendTo`), `:642` (`DragDrop`); `mde-files/src/backend.rs:736` (`RealBackend::send_to`→local→`DestinationUnreachable` :583); `mackesd/src/ipc/files.rs:185` (`send-to` verb → `SEND_TO_NOT_CONFIGURED`); `mackesd/src/orchestrator.rs`+`preflight.rs` (orphaned engine) | Facade / fit | **Send-To across the mesh is dead at every seam.** All six GUI entry points funnel to `Message::SendTo`, whose reducer discards the request (`_req`); no UI code calls `Backend::send_to`; `RealBackend::send_to` delegates to the local backend which hard-returns `DestinationUnreachable` for mesh targets; the daemon verb replies "not configured"; and a complete, tested `path_safety→preflight→orchestrator` Send-To engine exists but **no binary/Bus/sibling ever invokes it**. The SVC-5 three-bridge *exchange* can read but not send. | High | **FINISH** |
| **2** | `mackesd/src/workers/kdc_host.rs` (`PendingSends`; `ring`/`sms`/`clipboard`/`share` push, never drained); `LanTransport::send_to` called nowhere | Stub (documented) | KDC outbound is **enqueue-only** — the queue has no `pop`/`drain`, so "ring my phone" / "send file" (incl. the PD-3 L6 Devices-group buttons) build a correct `Packet` that never reaches the device. Documented as the pending `kdc_outbound` worker / 2-device bench. Inbound + pairing (mutual-TLS, cert-pin, battery) are real. | High | **FINISH** |
| **3** | `mde-workbench/src/panels/connect.rs:308,355-373`; `mackesd/.../kdc_host.rs` | Stub / mockup | The **Connected Devices** sidebar panel's `PeerAction` (Unpair/Ring/Find) is a no-op, and `load()` returns `Task::none()` so the roster is always empty. Its stated unblock — the `dev.mackes.MDE.Connect` D-Bus surface — was **retired** for a Bus responder, so the condition never arrives as written. (The live KDC roster already exists on `action/connect/devices`, which PD-3 L6 consumes.) | High | **FINISH** (rewire to the Bus) or **REMOVE** (drop nav entry) |
| **4** | `crates/platform/mde-cosmic-applet` (bin); `mackesd/Cargo.toml` `[package.metadata.generate-rpm]` assets | Packaging / fit | The cosmic-applet — **the §5 "Magic Mesh integrates via a cosmic-applet" integration** — is referenced by nothing downstream: absent from the RPM `assets`, from CI, and there is no cosmic-panel `.desktop` registration in `packaging/`. It builds into a void and would never reach a user's panel. | High | **FINISH** |
| **5** | `crates/shared/mde-disclaimer` (build.rs gate + `about.rs` display only) | Disclaimer / fit | The §5 DISCLAIMER **pre-flight accept gate** is build-time non-empty enforcement plus a display-only Help→About panel. There is **no runtime "I agree before use" / first-run / install-time consent path** anywhere. The accept requirement is cosmetically met at best. | High | **FINISH** |
| **6** | kickstart `%post` (`first-boot.txt` hint); no first-run RoleChooser crate/surface | Packaging / fit | The install-time deployment-role chooser (Lighthouse ⊂ Server ⊂ Workstation) is real on the **CLI/kickstart** path (`mackesd role-pin`, upgrade-only, fail-closed), but the **Cosmic first-run GUI chooser** the kickstart explicitly defers to **does not exist as code** (PKG-5's visual half). | High | **FINISH** |
| **7** | `mackesd/src/ipc/files.rs:121,139,158` (inbox/outbox/downloads `list` → `"[]"`) | Mockup-adjacent (honest) | The inbox/outbox/downloads file surfaces serve but return an empty list — the producer side ("AF-5") is genuinely absent. Honest (labels itself), but carries no real data. | Med | **FINISH** |

## Findings — convention violations (§1–§6)

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **8** | `mde-workbench/src/main.rs:134-136` + `single_instance.rs:25` | §2 (private D-Bus name) | LIVE `connection.request_name("dev.mackes.MDE.Workbench", DoNotQueue)` — a reintroduced MDE-private D-Bus **server** name for single-instance detection. §2 forbids new MDE-private bus names (only FDO `org.freedesktop.*` interop). | High | **FINISH** (move single-instance to the Bus / a lockfile) **or** sanction as the one documented exception |
| **9** | `mde-files/src/theme.rs:39-130` | §4 (parallel tokens) | A ~40-constant parallel token module (`rgb_hex(0x16,0x16,0x16)` … PF_BG/ACCENT/PF_INFO/MESH_PILL) whose `//!` claims to BE "the single source" for mde-files — directly contradicting §4's "Carbon tokens single-sourced in `mde-theme`." Evades the lint via byte-literal `rgb_hex(0x..)`. | High | **FINISH** (re-source from `mde-theme`, or §4 sanctions the module) |
| **10** | `mde-files/src/widgets.rs:825` (`tx_row`, render-path via `views.rs:828`) | §4 (raw literal) | Inline `Color::from_rgb(0x6f.../255, 0xb1.../255, 1.0)` — a raw Blue-40-ish literal on the render path, not even a defined token (closest `ACCENT_HI` is `0x78a9ff`). | High | **FINISH** (use an `mde-theme` token) |
| **11** | `mde-voice-hud/src/sip.rs` (`Algorithm::Md5`) | §3 (undocumented MD5) | SIP digest-auth uses MD5 per RFC 3261 (server-chosen; external-spec-mandated, no MDE security) — but SIP is **not** in §3's documented MD5-interop exceptions (only Subsonic auth + thumbnail cache naming are). | Med | **FINISH (doc)** — add SIP to §3 exceptions (or prefer SHA-256 when offered) |
| **21** | `AI_GOVERNANCE.md` §2 + §4 vs `install-helpers/` | Meta / process | §2 and §4 both claim "lint-gated" enforcement, but only `lint-mesh-boundary.sh` (§6) exists. The missing §4 (raw-hex/token) and §2 (private-bus-name) gates are *why* #8–#10 slipped in. | High | **FINISH** (stand up the two lint gates — highest-leverage, prevents recurrence) |

## Findings — unreachable code (REMOVE)

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **12** | `mackes-mesh-types/src/tag_predicate.rs`, `window_rules.rs`, `workspace_overrides.rs` (whole modules + lib.rs re-exports) | Unreachable | All three (Pred/evaluate/parse; WindowRule/WindowRulesFile; WorkspaceOverride/…) have **zero consumers** anywhere — MackesWorkstation-split leftovers (`window_rules`/`workspace_overrides` are desktop-WM concerns Cosmic owns now). | High | **REMOVE** |
| **13** | `mde-iced-components/src/lib.rs:300,426` (`ContextMenuItem` family + `context_menu_surface`) and `:212/225/237` (`overlay_white_on`/`overlay_color_on`/`with_alpha` re-exports via `panel_chrome.rs:41`) | Unreachable | No production caller (only the crate's own tests). mde-files has its own unrelated `ContextMenuItem`; `controls.rs`/`mde_theme` have their own `with_alpha`. **Keep `object_card`** — still consumed by `mde-workbench` `mesh_bus.rs:1151`. (The long-standing GUI-5 item.) | High | **REMOVE** (the dead widgets, not the crate) |
| **14** | `mackesd/src/workers/mesh_shunt.rs:141` (`MeshShuntWorker` struct + `impl Worker`) | Unreachable | The struct is never instantiated/spawned; the module's free fns (`publish_phones`/`collect_synthetic`/…) ARE used (inlined into `kdc_host.rs`). Dead wrapper, live helpers. | High | **REMOVE** (the struct) |

## Findings — doc drift / retired-surface live actions (FINISH)

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **15** | `mde-workbench/src/app.rs:885` | DocDrift / dead exec | LIVE `Command::new("mde").arg("settings")` spawns the **retired `mde` dispatcher** (gone post-pivot); the comment admits "targets the retired dispatcher path." | High | **FINISH** (retarget to the real settings surface / remove) |
| **16** | `mde-workbench/src/panels/repair.rs` (`ReloadCompositorClicked` → `labwc --reconfigure`) | DocDrift / dead action (§5) | A live Repair button dispatches `labwc --reconfigure` — labwc is the EOL'd shell; **Cosmic owns the desktop**. | High | **FINISH** (retarget to Cosmic / drop) |
| **17** | `mde-workbench/src/panels/{displays.rs, keyboard.rs, mouse.rs}` module docs | DocDrift (§5) | Docs say settings "apply them to the compositor (labwc)" — names labwc as the live compositor (stale; Cosmic). | Med | **FINISH (doc)** |
| **18** | `mde-files/src/picker.rs:7` | DocDrift (§4) | `//!` says the chooser "reuses … the same warm-dark theme" — replaced by Carbon Gray-100. | Low | **FINISH (doc)** |
| **19** | `mackes-transport/{lib.rs,peer_path.rs,…}` module docs | DocDrift (§1) | Substrate code is clean Nebula (`NebulaDirect`/`…Relay`/`…Https443`/`KdcTls`), but active docs still describe the relay as "Tailscale DERP" / "Tailscale's WireGuard endpoint set" as if current. | Med | **FINISH (doc)** — reword heritage analogies |
| **20** | `mde-workbench/src/panels/home.rs:894-901` | DocDrift (§2, minor) | Probes `dev.mackes.MDE.Connect` name-ownership though that D-Bus surface was retired; degrades gracefully but the name reference is stale + nothing in-tree owns it. | Med | **FINISH** (probe the Bus instead) |

## Counts

| Category | FINISH | REMOVE | OK / noted |
|---|:---:|:---:|:---:|
| Fit-for-purpose gaps (#1–7) | 7 | — | — |
| Convention §1–6 (#8–11, 21) | 5 | — | many OK (substrate Nebula-clean, crypto pinned, MPRIS/Subsonic/thumbnail MD5 sanctioned) |
| Unreachable (#12–14) | — | 3 | `object_card` kept; most "dead" candidates cleared as reachable |
| Doc drift (#15–20) | 6 | — | several prior items confirmed already-fixed |
| **Total** | **18** | **3** | — |

Clean on: `todo!()`/`unimplemented!()` (zero), §6 mesh/desktop boundary (lint passes), production-path mockups (none — `DemoBackend`/`demo_data` are test-only + contained), substrate transport (Nebula-native; no live Tailscale/Headscale/DERP/Gluster/OpenSSL), crypto values (Ed25519/AES-256-GCM/ChaCha/RSA-4096/rustls pinned).

## Fit-for-purpose verdict

**The mesh is real; the desktop product around it is the unfinished edge.** An operator can today stand up a no-fixed-center Nebula mesh, enroll peers hands-off, replicate via LizardFS, see the live peer directory + map, drive fleet reconcile, place real SIP calls, and stream music — all from real, reachable code with honest failure states. That is the hard part, and it is genuinely fit for purpose. What's missing is concentrated and nameable: (a) **moving files between peers** — the platform's headline file-exchange capability is a facade at every seam, and the phone hub can pair/listen but not act outbound; (b) **shipping** — the Cosmic-applet integration isn't packaged, the install role-chooser has no GUI, and the disclaimer accept gate is build-time-only, so the "one RPM + ISO" product isn't yet deliverable as designed; and (c) **two missing lint gates** that let a §2 private bus-name and a §4 parallel token module re-enter. None are deep architectural problems — they are wiring (#1–#3), packaging (#4–#6), and guardrails (#21). Close #1 (mesh file transfer) and #4–#6 (the shipping trio) and the platform crosses from "a working mesh with a desktop" to "a shippable secure-mesh desktop product."

## Suggested order of execution (when you choose to act)

1. **#1 — mesh file transfer (Send-To).** The headline capability. Wire the `orchestrator` Send-To engine to the `file-ops/send-to` verb, give `BusBackend`/`MeshBackend` a real transfer method, and connect the GUI `SendTo`/`DragDrop` reducers. The biggest single fit-for-purpose win.
2. **#4–#6 — the shipping trio.** Package the cosmic-applet (RPM asset + cosmic-panel `.desktop`), build the Cosmic first-run role-chooser GUI, and add a runtime DISCLAIMER accept gate. Together these make the one-RPM-plus-ISO product actually deliverable.
3. **#21 — the two missing lint gates** (§4 raw-hex, §2 private-bus-name). Cheap, prevents recurrence; then fix the #8–#10 violations they would have caught.
4. **#2 / #3 — KDC outbound drainer + the Connected Devices panel rewire** (related; the panel's actions become real once outbound drains).
5. **#12–#14 — delete the dead surfaces** (mesh-types leftover modules, mde-iced-components dead widgets, the `MeshShuntWorker` struct).
6. **#15–#20 — doc drift + dead retired-surface actions** (mechanical; clears the last labwc/`mde`-dispatcher/warm-dark/Tailscale-DERP references).
