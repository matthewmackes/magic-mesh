# Reconciliation working spec (master → farm-autoscale-plan, → 11.2.0)

> Auto-generated 2026-06-29 by the divergence-mapping workflow (8 subsystem agents + synthesis).
> This is the execution spec for unifying the two divergent lines. See POSTMORTEM-line-divergence.md for WHY.

# MCNF Branch Reconciliation Plan — origin/master → origin/farm-autoscale-plan

## 1. RECOMMENDED STRATEGY: base-on-canonical + port-extras (NOT a bidirectional merge)

Take `origin/farm-autoscale-plan` (11.1.0, fleet-proven at 11.0.14) as the trunk wholesale, then cherry-port a small, explicit set of master-unique features. Do **not** attempt a 3-way / bidirectional merge.

Why this is strictly lower-risk:
- **Master is a pre-SUBSTRATE-V2 fork.** It still carries the dead LizardFS plane in 7 of 8 subsystems. A line-merge would silently resurrect LizardFS (the retired FUSE wedged-mount failure class) onto the live fleet.
- **A naive "take master" on three files is actively dangerous and silent:** `mackes-mesh-types/src/ddns.rs` (deletes the leak-proof DNS reconcile core + kill-switch sentinel), `mackesd/src/ca/sign.rs` (reverts the live-caught MULTI-LH-IP-ALLOC collision fix), `packaging/repo/RPM-GPG-KEY-magic-mesh` (reverts the RSA-4096 subkey → breaks Fedora 43 signing).
- **The big diffs don't merge cleanly anyway:** `bin/mackesd.rs` (~2163-line diff), `datacenter.rs` (3432 vs 2422), `home.rs`/Front Door launcher fork. These must be taken as canonical-whole, not line-merged.
- Master's genuine new value is narrow and well-localized — it ports far more safely as deliberate grafts than as merge resolutions.

---

## 2. THE PORT LIST (master-unique work worth bringing onto canonical)

### A. Clean ports (additive, low/no conflict)
| Feature | Files | Notes |
|---|---|---|
| XPA-7 clone self-enroll (cloud-init join seed) | `mackes-xcp/src/lib.rs` (build_join_seed / mackesd_join_argv) | Pure, additive |
| DDNS published-state model | `mackes-mesh-types/src/ddns.rs` (PublishedRecord/DdnsPublished/load_published) | Confirmed non-colliding; grafts on top of canonical decision engine |
| VPN-GW-6 published exit state | `mackes-mesh-types/src/vpn.rs` (TunnelExit/VpnExitState/exit_state_*) | Take ONLY this block — leave out master's RouteScope/EgressRoute |
| Tofu prod-arm gate + run-log | `ipc/dc_common.rs` (new), `ipc/tofu.rs` (tofu-arm, tofu-runlog) | tofu.rs ACTION_VERBS 4→6; additive |
| ipc DDNS list-records + sync-now | `ipc/ddns.rs` | Keep ALONGSIDE canonical's record-status, don't replace it |
| vm-bulk + vm-resume | `ipc/datacenter.rs` | Re-pin onto canonical dispatcher |
| MEDIA-9 ingest helper | `install-helpers/mcnf-music-ingest.sh` | Reuses canonical's setup-media-navidrome creds; clean |
| DATACENTER-22 GPU passthrough | `install-helpers/setup-workstation-passthrough.sh` | Additive; flag hardware-unverified |
| ABOUT-OSS page + 7 logos | `mde-workbench/src/panels/about.rs`, `assets/oss/*.svg` | Self-contained |
| NotifyCenter autostart | `packaging/autostart/org.magicmesh.NotifyCenter.desktop` | Verify `mde-notify-center` binary name matches canonical first |
| DRAIN-7 governance paragraph | `AI_GOVERNANCE.md` | Documents a guard canonical already ships in code |
| MEDIA-9 doc section | `docs/design/media-lighthouse.md` | Append; gate on the ingest helper landing |

### B. Ports requiring conflict resolution / re-expression
| Feature | Files | Resolution required |
|---|---|---|
| XPA-4 clone VIF-MAC reset | `mackes-xcp/src/lib.rs` (reset_vif_macs, mac_for_clone, VifInfo) | Adds a `Hypervisor::reset_vif_macs` **trait method** → every impl + mock must implement it. Also reconcile rename `argv_set_memory`→`argv_memory_set` touching `workers/xcp_provision.rs` |
| L1043 bus consumer-cursor retention safety | `mde-bus/migrations/0001_init.sql` (consumer_cursors), `mde-bus/src/retention.rs`, `correlate.rs` | Additive migration on top of canonical's LARGER retention.rs; reconcile alongside correlate.rs/publisher.rs divergence |
| KeyedListReveal motion primitive | `mde-theme/src/animation.rs` + per-panel wiring | Port the struct cleanly; **per-panel wiring needs per-panel review** (panels heavily diverged/ahead on canonical). KEEP canonical's `mde-theme/src/hue.rs` (master deleted it) |
| DATACENTER-23 dr-ca-backup + dr-rebirth | `ipc/host_ops.rs` verbs + `dc_rbac` gating | Port the panel-reachable verbs but **re-point them at canonical's DR v2 scripts** (`dr-ca-bundle.sh`/`dr-reconstitute.sh`), NOT master's redundant scripts (see DROP). Cross-subsystem |
| DRAIN-4 `--readiness` preflight | `install-helpers/farm-reconciler.sh` | Graft the subcommand onto canonical's body; adapt to the XAPI :443 + etcd-durable gate — do NOT bring master's reverted XO-websocket apply_gate |
| XEN-194 4th build pool | `infra/tofu/xen-xapi/providers.tf` (x194 alias), `infra/tofu/main.tf` (dom0 entry), `infra/ansible/inventory.ini` | Provider alias + dom0 entry + inventory port verbatim. **Re-express the VM as a `local.dom0` entry in the 225-line for_each model — do NOT cherry-pick master's `build-vms.tf`** (see hotspots) |
| DDNS-EGRESS-5 worker improvements | `workers/ddns.rs` | Base on canonical; graft sync-nudge + stdin-token + on_down-sentinel only (reconcile TunnelReport vs VpnExitState source model first) |

### C. NEEDS_REVIEW — operator decision before porting
- **Two-role RBAC + tamper-evident audit** (`ipc/dc_rbac.rs`, 682 lines): canonical deliberately rescoped DATACENTER-7 to a `confirm:true` single-operator flat-trust model. Only port if multi-principal access is actually wanted. **Lift the audit-deny-record half regardless** of the role decision.
- **dc_power live wake-progress driver**: port the progress-driving logic but re-pin onto canonical verb names (`wake-eta`/`ipmi-power`, not `power-eta`/`power-ipmi` — the GUI is pinned to canonical names).
- **HA-5 ha_monitor quorum-status publish**: port the concept onto the SUBSTRATE-V2 etcd plane; dedupe leader-change against canonical's `etcd_watch`. Do NOT import the poll-based worker wholesale.
- **dc_network / dc_storage responder verbs** (net-create, pif-config, ipdns; sr-list/sr-destroy/iso-list): decide **per-verb** — these overlap canonical's host-net/gateway-dhcp and storage_ops; keep canonical's vdi-create + scheduler-worker.
- **DATACENTER-11 xe VM lifecycle argv** (`mackes-xcp`): overlaps canonical's parallel virsh+rsync `compute_migrate` worker. **Pick the canonical hypervisor migration mechanism first** (fleet is XCP-ng → xe may be the better fit, but it's a shipped-worker overlap).
- **xcp_provision XPA-4/7/XCP-6**: confirm the worker is still the live spawn path (DEVOPS-AUTOMATION-REBUILD may now provision via tofu/reconciler). MAC-reset + capacity-advert are safe regardless.
- **L656 Action Center read/cleared persistence** (`mde-notify/src/lib.rs`): confirm canonical truly lacks it before grafting (shared notifications.rs persistence is already identical).
- **motion-language.md / motion-system.md / §4 rewrite**: coupled to master's `lint-motion-tokens.sh` + mde-theme module split. Only adopt if master's motion lint/code wins; otherwise keep canonical's motion-guide.md + lint-motion.sh.
- **BUILD-ENVIRONMENT.md 4th node + BIGBOY resize**: splice host-table rows onto canonical's §10/3-VM base **after** operator confirms live hardware; fix master's intro/table contradiction.

### ⚠️ Port-list correction (cross-subsystem inconsistency caught)
The infra report recommends porting `automation/testbed/test-lighthouse-mount.sh` "verbatim" — **reject that as-written.** That test founds QNM-Shared and asserts `/mnt/mesh-storage` is a **live writable FUSE mount**, which does not exist on the canonical Syncthing plane. Master's own `unwedge-lizardfs.sh` concedes the failure mode "goes away entirely" under etcd+Syncthing. Either drop it or rewrite it for the plain-dir Syncthing plane — do not add a FUSE-mount assertion to the canonical testbed.

---

## 3. DROP LIST (master work redundant with fleet-proven canonical — do not re-merge)

**LizardFS / SUBSTRATE-V2 regressions (drop everything):**
- `workers/meshfs_worker.rs` (2321 lines) + all LH-JOIN-QNM-1 stray-write guards (`validation_suite.rs`, `netdata_aggregator.rs`, `hardware_probe.rs`, `nebula_ca_backup.rs` meshfs_snapshot stitch, `ssh_pubkey_gossip.rs`)
- `mackesd/src/meshfs/` (mod/snapshot/headroom) + `lib.rs pub mod meshfs`; the `meshfs_snapshot` field / schema_version 3 in `ca/backup.rs`; QNM-Shared LizardFS provisioning in `bin/mackesd.rs`
- `install-helpers/{mesh-install-lizardfs,vendor-lizardfs-rpms,unwedge-lizardfs,setup-qnm-shared,qnm-mount}.sh`
- All master LizardFS/QNM-Shared/allow_other doc+unit regressions: `AI_GOVERNANCE.md`, `docs/architecture.md`, `COMPLIANCE.md`, `xcp-ng-integration.md`, `mackesd.service`, `etcd.service`, `tmpfiles/magic-mesh.conf`

**Parallel re-implementations (keep canonical):**
- ipc CONNECT-9 DdnsOp decoupling (`connect.rs`) → keep direct-config path
- ipc dc_provision testmesh-spin/teardown → keep canonical testbed-up/down/list (async, n≤12, with list)
- `workers/media_navidrome.rs` → keep canonical's 3 granular workers (navidrome_supervisor + media_registry + music_autoconfig)
- `workers/vpn_health.rs` → keep canonical `ipc/vpn_health.rs`
- mesh-types VPN-GW-4 routing in `vpn.rs` (RouteScope/EgressRoute) → keep `vpn_egress.rs`; drop `role_is_lighthouse()`
- GUI: `home.rs` Workbench Overview (3289 LOC), `mde-apps-applet.rs` (2701 LOC), `services_map.rs` (SVC-VIEW → keep all_services.rs), `Role::LighthouseMedia` enum (keep capability-tag), master's subset datacenter.rs
- infra DR scripts `dr-ca-backup.sh`/`dr-rebirth.sh` + master's `dr-backup.sh` hunk → keep DR v2 (`dr-ca-bundle.sh`/`dr-reconstitute.sh`)
- `install-helpers/check-worktree-isolation.sh` → keep canonical `assert-own-worktree.sh`

**Never accept:** master's `RPM-GPG-KEY-magic-mesh` (older short key), master's reverted farm-reconcile units, master's applet `.desktop`, master's `mackes-mesh-types/src/peers.rs`/`lighthouse.rs` (drop the media wire field + revive dead-plane docs + delete roster_with_self).

---

## 4. HIGH-RISK HOTSPOTS (where merging can silently regress fleet code)

1. **`mackes-mesh-types/src/ddns.rs`** — THE TRAP. Blind "take master" deletes the DNS-leak safety core (plan_action/SourceState/SENTINEL_ADDR/kill-switch coupling) + CONNECT-9 self-naming. Resolution = **union**: keep ALL canonical logic, graft only master's PublishedRecord/DdnsPublished.
2. **`mackesd/src/ca/sign.rs`** — keep `allocate_overlay_ip_excluding` + `extra_taken`; taking master's plain signature reintroduces the live-caught lighthouse IP collision. Cascades to `ca/epoch.rs`.
3. **`bin/mackesd.rs`** (~2163-line diff) — apply canonical wholesale. A line-merge risks losing CA-key delivery gating, lighthouse add/retire, etcd-join, secret CLIs.
4. **`infra/tofu/xen-xapi/build-vms.tf`** — the `moved{}` blocks are load-bearing (CONTRADICTION-2 in-file warning). A wrong resource address reads as destroy+recreate and **destroys the 3 live adopted build VMs**. Re-express XEN-194 onto the for_each model with `tofu state mv` discipline; never cherry-pick master's 51-line hardcoded shape.
5. **`packaging/repo/RPM-GPG-KEY-magic-mesh`** — signing-key conflict; master's key breaks Fedora 43 RPM verification. Hard-pin canonical.
6. **`ipc/datacenter.rs` / `host_ops.rs` / `dc_power.rs`** — both-partial, different verb names + different module decomposition (master split storage/network into dc_*; canonical folds them). Base canonical, graft master's verbs onto canonical's dispatcher + verb names.
7. **`storage_ops.rs` (canonical) vs `dc_storage.rs` (master)** — neither is a superset; reconcile verb-by-verb (canonical has vdi-create; master has sr-list/destroy/iso-list).
8. **`mde-bus/src/retention.rs` + `correlate.rs` + `publisher.rs`** — all three diverged; the L1043 port is schema-touching and spans all three.
9. **`mackes-xcp` Hypervisor trait** — adding `reset_vif_macs` forces every impl + mock to implement; build won't compile until all are updated.
10. **GUI launcher fork** — Front Door (canonical, sole launcher) vs home-overview + apps-applet (master). Not a textual merge; keep Front Door, optionally fold master's capability-list content into it.
11. **`install-helpers/farm-reconciler.sh`** — divergence centered on apply_gate (etcd/XAPI vs reverted XO); keep canonical body, graft `--readiness` only.

---

## 5. EFFORT, RISK, VERSION

**Effort:** Moderate. The *decision* is clean (canonical base), and ~85% of files are "keep canonical, drop master" (zero merge work). Real work = ~12 clean ports (hours) + ~7 conflict-resolution ports + ~9 operator-gated NEEDS_REVIEW items. Estimate **1–1.5 weeks** of focused integration if most NEEDS_REVIEW items are green-lit; ~3–4 days for the clean-ports-only minimum (XPA-4/7, types, ABOUT-OSS, MEDIA-9, passthrough, tofu prod-arm, dr-verb re-point, XEN-194, L1043). Re-run full build/test/lint per subsystem; `cargo clean -p` the changed workspace crates on the integration slot (stale-rlib hazard).

**Risk:** **MEDIUM** if executed as canonical-base + targeted grafts. **HIGH** if anyone attempts an auto/3-way merge — dominant failure modes are all silent: LizardFS resurrection, IP-collision regression, DDNS leak-core deletion, signing-key break, and the build-vms.tf live-VM destroy.

**Version bump:** Canonical is **11.1.0**. The net-new master capabilities being added (XPA-4/7 MAC-reset + self-enroll, L1043 retention safety, tofu prod-arm gate, DATACENTER-23 dr-rebirth verbs, MEDIA-9 ingest, GPU passthrough, ABOUT-OSS, KeyedListReveal) are a feature increment, not a breaking change. **Recommend 11.2.0** for the reconciled trunk.