# Needs-Operator — operator-blocker sink (re-keyed to WL-* IDs)

> **NOT AN ACTIVE TRACKER — see [`docs/platform/WORKLIST.md`](platform/WORKLIST.md).**
> The single authoritative active worklist is `docs/platform/WORKLIST.md` (the
> 18 active **WL-\*** epics). This file is **only** the drain loop's
> operator-blocker *sink*: `install-helpers/park-blocker.sh` and
> `automation/drain/park-worklist-item.sh` append parked units here under
> "Parked by the drain loop" so the loop never stalls. Every blocker is *worked*
> under its **WL-\*** epic, not here. The re-key map below points each historical
> (pre-2026-07-16, old-ID) entry to the epic that now owns it. Do **not** treat
> this file as a parallel roadmap.

The verbose 2026-06-27 operator-queue detail (the exact cred/host/decision each
blocker needs) is preserved verbatim at
[`docs/worklist-archive/2026-07-19-needs-operator-detail.md`](worklist-archive/2026-07-19-needs-operator-detail.md).
See [`docs/worklist-archive/README.md`](worklist-archive/README.md) for the
archive's role.

## Re-key map — old ID → owning WL-\* epic (2026-07-19)

Each row: the old queue ID (left) is now tracked by the WL-\* epic (right). A
`Status: Blocked` or `Status: Awaiting testing` epic is where the
operator or testing-wait gate lives; check that epic's `- Status:` and
acceptance in `WORKLIST.md` for the current state.

| Old queue ID | Owning WL-\* epic | Notes |
|---|---|---|
| BUILD-PLATFORM-1 (cross-node cache hit) | **WL-BUILD-002** | Farm shared cache + fresh-farm bootstrap; needs live farm nodes + sccache. |
| BUILD-PLATFORM-5 (per-feature Bus pass/fail) | **WL-BUILD-003** | Promotion / version-matrix / gate reporting; needs nightly on live infra. |
| BUILD-PLATFORM-6 (chaos + reboot-recovery) | **WL-TEST-002** | Crown-jewel integration harness (real etcd/Nebula, multi-node chaos). |
| COMPUTE-DISCOVERY (unified services view) | **WL-FUNC-008** | Unified services view (canonical + probe + VM-internal). |
| DATACENTER-3 / DS-8 (mesh secret store) | **WL-SEC-003** | Secret-store distribution + scoped decryption roots. |
| DATACENTER-23 (control-plane DR) | **WL-CRIT-004** | Control-plane DR backup + guided rebirth. |
| FED-RUNTIME (federation.yaml consumer) | **WL-SEC-002** | Federation runtime enforcement + two-mesh accept. |
| FED-XMESH (cross-mesh accept envelope) | **WL-SEC-002** | Same epic; needs the pairing-model design decision. |
| FED-GUI (panel no-ops + guards) | **WL-SEC-002** | Same epic; resolve with FED-RUNTIME. |
| LIGHTHOUSE-VARMOUNT (reboot /var + mackesd) | **WL-RUN-003** | Lighthouse join/add/retire; live-verify after droplet reboot. |
| MEDIA-2/3/4/6/9 (Navidrome + Spaces path) | **WL-RUN-004** | Media lighthouse production service, failover, upload path. |
| MEDIA-10 (redundancy + live verify) | **WL-RUN-004** | ARCHIVED: RESOLVED 2026-07-01 (active-active LH1/LH2); folded into WL-RUN-004 acceptance. |
| OW-3 / OW-4 / OW-5 (mesh-create / join / net) | **WL-SEC-001** | Fresh-node enrollment bootstrap + final join path. |
| OW-7 (spawn-lighthouse, cloud) | **WL-RUN-003** | Push-button add lighthouse; needs a DO API token. |
| OW-8 (first-desktop) | **WL-CRIT-001** | Mesh VDI console broker end-to-end (`session_broker`). |
| OW-11 (service-add: Music / Voice) | **WL-RUN-004** | Media service-add; Voice SIP has no separate active epic. |
| OW-12 (headless-WS kickstart / ISO) | **WL-BUILD-001** | Immutable bootc/ISO/RPM release gate; live-boot + `/release` gated. |
| DAR-19 (genesis-fresh enroll layer) | **WL-SEC-001** | Fresh-box bootstrap-enroll (connects to LH-JOIN-QNM-1). |
| DAR-34 / DAR-49 (control-plane golden IaC) | **WL-BUILD-002** | Bake enroll-ready golden so `tofu apply` yields a joinable VM. |
| ROUTER-6 (2nd-appliance migration) | **WL-RUN-006** | Router discovery + firewall commit-confirm; DEFERRED-YAGNI until a 2nd appliance. |
| NAMING-2 (VM vocabulary + panel badging) | **WL-ARCH-002** | Cloud/Datacenter resource surface; Q38 two-path scope needs an owner. |
| 12.1 release (KEEP ACCUMULATING) | **WL-BUILD-001** | Release gate; `/release` is operator-gated. |

### Archived with a disposition (no owning epic)

- **E12-9 remote audio** — superseded by the 2026-07-31 operator decision:
  audio is a first-class production requirement, including node-to-node
  streaming. The former WON'T-DO disposition is historical evidence only; the
  active requirements live under `WL-FUNC-011` and `WL-CRIT-006` in
  [`docs/platform/WORKLIST.md`](platform/WORKLIST.md).
- **MOTION-TRANS-4 / MOTION-PERF-4** — WON'T-DO (operator 2026-07-03): their
  acceptance targets the retired iced/Cosmic compositor; re-doing the polish on the
  egui/Construct shell would be net-new work, not completion.
- **NAMING-1** — RESOLVED 2026-07-18 (brand sweep, tracked under `WL-UX-004`, now
  closed): "Construct" is the visible product name / 12.x codename; `magic-mesh`
  stays the package/repo/infra id.
- **Standing authorization (operator 2026-07-03)** — not a queue item: standing
  prod-SSH + XCP cloud create/delete + maintenance window (DAR DevOps rebuild) and
  the live Construct VDI test bed. Recorded here for context only.

## Parked by the drain loop (DRAIN-5)

Units the drain loop parked automatically (a live-infra/artifact/gate blocker it
could not clear from a build). Each needs an operator/live action; each is worked
under the WL-\* epic named in the re-key map above, not as an independent ID.

- **E12-9-audio** (parked 2026-07-01) — historical parked item superseded by the
  2026-07-31 first-class audio requirement. The implementation must select a
  supported VDI/mesh audio transport and close the live `.15` playback/capture
  and node-to-node streaming gates; it is no longer a WON'T-DO requirement.

- **mde-shell-egui pre-existing test reds** (recorded 2026-07-21, WL-FUNC-011
  Phase-2) — RESOLVED 2026-08-19 against the current tree, verified by source
  inspection: (1) `system::tests::every_section_is_reachable_exactly_once` now
  enumerates all fifteen `SettingsSection` variants and asserts the taxonomy
  length is 15 (`system/tests.rs`, `system/mod.rs`); (2)
  `tests::shell_remote_sessions_fallback_mounts_for_bare_non_desktop_workspaces`
  and (3) `tests::shell_remote_sessions_fallback_request_uses_shell_transition`
  now drive `Surface::Browser` — a surface genuinely outside the menubar-bearing
  set — instead of the stale `Surface::Files` expectation (`main.rs`). No
  operator action remains.

- **DESIGN RULING NEEDED — browser chrome light-vs-dark** (recorded 2026-07-21,
  `/polish`) — MOOT 2026-08-19: the VM-only Browser cutover (WL-ARCH-008)
  extracted the CEF host browser; `web/chrome_ui/` and its 31 `CHROME_*`
  constants no longer exist — the Browser surface is the thin typed `browser-vm`
  controller in `crates/desktop/mde-shell-egui/src/web/mod.rs`, and guest
  Chromium owns browser chrome per the platform boundary. No ruling is required.
