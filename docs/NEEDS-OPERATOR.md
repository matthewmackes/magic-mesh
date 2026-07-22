# Needs-Operator — operator-blocker sink (re-keyed to WL-* IDs)

> **NOT AN ACTIVE TRACKER — see [`docs/platform/WORKLIST.md`](platform/WORKLIST.md).**
> The single authoritative active worklist is `docs/platform/WORKLIST.md` (the
> 43 reconciled **WL-\*** epics). This file is **only** the drain loop's
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
`Status: Blocked` epic is where the operator gate lives; check that epic's
`- Status:` and acceptance in `WORKLIST.md` for the current state.

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

- **E12-9 remote audio** — WON'T-DO (operator 2026-07-03): avoids an ironrdp bump
  on a pinned dep. The local-audio remainder (QEMU/libvirt `<audio type='pipewire'>`
  into the E12-16 mixer) is a design-doc item in
  [`docs/design/e12-9-10-libvirt-rescope.md`](design/e12-9-10-libvirt-rescope.md),
  not an active WL epic.
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

- **E12-9-audio** (parked 2026-07-01) — remote audio needs an ironrdp RDPSND/audio
  virtual-channel API the pinned version doesn't expose. Disposition: WON'T-DO for
  remote audio (see "Archived with a disposition" above); local-audio path lives in
  `docs/design/e12-9-10-libvirt-rescope.md`.

- **mde-shell-egui pre-existing test reds** (recorded 2026-07-21, WL-FUNC-011
  Phase-2) — 4 `cargo test -p mde-shell-egui` tests fail on the branch base
  (`a698771d`, the 20-surface tree) *independent of* the Communications surface
  cutover (verified by a base-tree run: 1615 passed / 4 failed). They are NOT
  introduced by the cutover and are left untouched; triage separately:
  1. `system::tests::every_section_is_reachable_exactly_once` — the SettingsSection
     taxonomy has 14 sections but the test asserts 13 (a settings section was added
     without updating the count/list). `system/tests.rs`.
  2. `tests::car_home_tiles_and_default_key_bindings_cover_the_vehicle_apps` — the
     default car keymap resolves letter key `A` to `Some(GoAirspace)` where the test
     asserts `None` (the Airspace vehicle app's binding drift). `main.rs`.
  3. `tests::shell_remote_sessions_fallback_mounts_for_bare_non_desktop_workspaces`
     and 4. `tests::shell_remote_sessions_fallback_request_uses_shell_transition` —
     both drive `Surface::Files`, but `surface_needs_remote_sessions_fallback` now
     lists `Files` in the menubar-bearing (no-fallback) set (the crash-fix that
     added Media/Files/Terminal/… to that set), so the fallback control never mounts
     and the tests' expectation is stale. `main.rs`.

- **DESIGN RULING NEEDED — browser chrome light-vs-dark** (recorded 2026-07-21,
  `/polish`) — `crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs` deliberately
  mints 31 light-Material `CHROME_*` constants ("Chromium/Chrome Refresh light roles,
  mirrored as local egui tokens so every Browser surface can stay on the stock Chrome
  palette instead of inheriting the darker shell chrome"). This is a DELIBERATE
  Chrome-fidelity choice for the CEF browser, but it conflicts with Quasar design
  **lock #1 (dark only)** and is the sole source of the 31 shell style-leak-grep hits.
  Ruling needed — pick one, then `/polish` can act:
  (a) KEEP stock-Chrome-light → add `web/chrome_ui/` to the style-leak grep's
      exclusion list (same category as the VDI decoders / term palette = deliberate
      data/fidelity, not look-to-drain); or
  (b) CONFORM to dark → recolor the browser chrome to the `mde-egui` Quasar-dark
      tokens (a real visual change to the browser surface).
  Until ruled, `/polish` holds the shell — it is NOT a blind drain.
