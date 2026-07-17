# NEEDS-OPERATOR — worklist items blocked on live infra / external / decision

_Generated 2026-06-27 from the worklist reconcile (wf_0cfa1277). These items have complete or partial code but cannot be closed by a coding agent — they need the listed live resource, external service, or operator decision._


## BUILD-INFRA

- **BUILD-PLATFORM-1: cache hit verification; a crate built on node A then B shows hit, fresh ** — _needs:_ This is a RUNTIME-OBSERVABLE acceptance: requires live build farm nodes (172.20.0.50/.90/.130/.170 — 4 build VMs; see install-helpers/farm-topology.sh) with sccache.yml applied, a fresh RPM build on one node to populate cache, second build on different node 
- **BUILD-PLATFORM-5: per-feature pass/fail on Bus (event/test/feature/<id>), red feature name** — _needs:_ test-feature.sh line 60+ publishes to Bus (mde-bus) + nightly.sh would aggregate + report. But this depends on BUILD-PLATFORM-5 test harness actually running nightly on live infra, which is BLOCKED on
- **BUILD-PLATFORM-6: chaos test — destroying a lighthouse causes no fleet-wide FUSE wedge, fa** — _needs:_ test-stability.sh lines 46-50+ implement chaos: shut down node B via xe vm-shutdown, verify node A's mackesd stays healthy (no uninterruptible procs, load sane). Code-complete. But requires live multi
- **BUILD-PLATFORM-6: reboot-recovery test — node reboots and mounts/overlay self-heal (BOOT-R** — _needs:_ test-stability.sh implements reboot via xe vm-reboot (code-complete). But BLOCKED on live mesh (same as soak/chaos above). Additionally, self-heal assertions (mounts + overlay come back) require inspe

## COMPUTE-DISCOVERY

- **Single 'Services across the mesh' view unioning canonical + probe-discovered + VM-internal** — _needs:_ A new unified panel to display ALL services (Published Services canonical-7 + Discovered Hosts from nmap + VM-internal) does not exist. The Published Services panel (line 1130, done) shows the canonic

## DATACENTER

- **DATACENTER-3: DS-8 mesh secret store holds DC creds** — _needs:_ automation/secrets/mcnf-secret.sh built; XAPI + DO creds resolve from etcd+age store (proven); UniFi cred pending (coupled to DC-14); store replication to other live nodes + guided-rebirth remain live
- **DATACENTER-23: control-plane DR (encrypted backup + one-click restore)** — _needs:_ automation/dr/dr-backup.sh + restore built + round-trip verified live; dr_scheduler worker + RPC + Overview button done. Remaining: Nebula CA + off-fleet push target + guided restore-rebirth (operator

## FEDERATION

_(2026-06-29 — surfaced during the workbench critical-bug drain. The `accept` auth-bypass itself is **fixed**: `cmd_accept` now consumes a single-use, unexpired, unconsumed local mint envelope — `crates/platform/mde-bus/src/cli/federation.rs`, farm-tested. These are the larger gaps that need a decision, not a build.)_

- **FED-RUNTIME: `federation.yaml` has no runtime consumer** — _needs:_ design/build decision. `mde-bus federation accept/grant/revoke` write `federation.yaml`, but nothing in `mackesd`/`mde-bus` reads it at runtime — the topic grants/exclusions have **zero live effect** (no bus-federation worker enforces cross-mesh topic flow). Either build the enforcement worker or the pairing surface is decorative.
- **FED-XMESH: cross-mesh accept has no envelope on the accepter** — _needs:_ design decision + the **missing** design doc. `accept` now (correctly) requires a local mint envelope; a true two-mesh pairing (mint on A, accept on B) has none on B. Decide the model (replicate the secret / handshake exchange / keep same-root) and author `docs/design/v1.0-federation-pairing.md` (cited everywhere, does not exist).
- **FED-GUI: panel-side no-ops + missing guards** (workbench audit) — _needs:_ operator decision. `mesh_federation.rs`: "Apply grants" writes `federation-pending-grant.yaml` that nothing reads (silent no-op reported as success); `rotate` only bumps the `established` timestamp (no real CA cross-sign); GUI writes are non-atomic vs the CLI's temp+rename; no confirm before destructive Revoke; accept label hardcoded. Resolve with FED-RUNTIME.

## LIGHTHOUSE-BOOT

- **LIGHTHOUSE-VARMOUNT acceptance bullet 3: verify on rebooted droplet mackesd active without** — _needs:_ Requires live infra verification on a real DO lighthouse after it reboots. The fix is code-complete and baked into packaging; verification is operator-gated (manual droplet provision + reboot).

## MEDIA-LIGHTHOUSE

- **MEDIA-2: DO Spaces 100 GB bucket + S3 creds as a leader-managed mesh secret** — _needs:_ Operator-gated. WORKLIST itself states: 'DO Spaces keys are console-minted; no S3 keys/rclone config exist on the dev host — the bucket + sealed key-pair require an operator action in the DO console (
- **MEDIA-3: Navidrome container worker on Lighthouse_Media** — _needs:_ Three blockers: (1) Lighthouse_Media role does not exist (MEDIA-1); (2) no navidrome mackesd worker exists; (3) the live Subsonic-API acceptance needs a live Lighthouse_Media node + MEDIA-2 bucket/key
- **MEDIA-4: mount the Spaces bucket into the container as /music** — _needs:_ Blocked on MEDIA-2 (live bucket + keys). Substrate (setup-media-navidrome.sh) implements rclone mount at $STATE/library with VFS cache, bind-mounted into container at /music:ro; two acceptance bullets
- **MEDIA-6: shared service account auto-provisioning** — _needs:_ Blocked on MEDIA-2 (live bucket) + live Lighthouse_Media node. Only the env-var half exists in the unpackaged helper. Idempotent account creation (first-start provisioning), durable shared-playlist wr
- **MEDIA-9: content ingestion — operator upload to the bucket** — _needs:_ Blocked on MEDIA-2 (live bucket) + MEDIA-3 (running Lighthouse_Media). No upload path or rescan trigger wired. Every acceptance (upload via rclone / mesh Files surface, rescan refresh, tracks appear i
- **MEDIA-10: redundancy + live verification on DO** — ✅ **RESOLVED 2026-07-01** (operator DO Spaces creds). LH1 (10.42.0.1) + LH2 (10.42.0.2) both serve `mcnf-mesh-media` active-active (Subsonic 200 each); `music.mesh` round-robins both; kill-one resilience proven (controlled+reversible: LH2 down → LH1 keeps serving → LH2 restored). Residual is only a user-visible-gap *metric* (needs a client mid-stream) + fresh-node auto-config (MEDIA-6/8), not a blocker.

## Parked by the drain loop (DRAIN-5)

Units the drain loop parked automatically (a live-infra/artifact/gate blocker it could not clear from a build). Each needs an operator/live action.
- **E12-9-audio** (parked 2026-07-01) — remote audio needs an ironrdp RDPSND/audio virtual-channel API that the pinned version doesn't expose; needs an ironrdp bump or a custom SVC decoder — operator/design call

## BUCKET-A onboard-seam live-verify (2026-07-01)

The onboard verbs are on the live fleet (11.3.1). The seams verifiable with prod-SSH
alone are **DONE + LIVE-VERIFIED** (OW-10 self-test, OW-13 recovery, OW-4 invite-issue —
see their worklist markers; 3 real bugs found + fixed, incl. the security-relevant
Nebula-Cert-V2 fingerprint fix shipped as 11.3.1). The remaining seams each need one
operator-provided cred/host — drop any and the loop wires + live-verifies it:

- **OW-4 join-half / OW-3 mesh-create / OW-5 network bring-up** — _needs:_ a **fresh
  throwaway node** to enroll/found/configure (spinning one itself needs a hypervisor
  or a DO droplet — the build VMs can't be used without disrupting their farm role).
- **OW-7 spawn-lighthouse (cloud)** — _needs:_ a **DigitalOcean API token** so
  `LiveProvisioner` can push-provision + enroll a droplet.
- **OW-8 first-desktop** — _needs:_ a reachable **Nova/Heat placement** (the mackesd
  `openstack` worker converged on the node) + a **golden image in the Glance catalog**
  so `LiveNovaPlacement` can place → boot → open the broker session, plus the live
  Bus/DRM seat leg. *(Corrected 2026-07-11: **QC-15 deleted mde-kvm/cloud-hypervisor
  outright** — `first_desktop.rs` now drives `workers::session_broker::LiveNovaPlacement`,
  not a cloud-hypervisor api-socket + golden disk. Cf. the WORKLIST OW-8 supersession
  note and the E12-9 remote-audio DESCOPED correction below.)*
- **OW-11 service-add** — _needs:_ **DO Spaces creds** (Music → Navidrome, overlaps
  MEDIA-2) + an **external SIP account** (Voice) so `LiveServiceApply` can provision /
  register.
- **OW-12** (parked 2026-07-03) — Quasar/headless WS kickstart authored (packaging/kickstart/magic-on-quasar.ks, bash -n + shellcheck clean); remaining acceptance is LIVE-BOOT-GATED (boot the ISO to confirm display + headless WS onboard) + the .iso cut is OPERATOR-GATED (/release, incl. RPM signing + bootc registry publish)

## Scope decisions (operator, 2026-07-03)
- **E12-9 remote audio DESCOPED** — remote RDPSND audio is WON'T-DO for the current release (avoids an ironrdp bump on a pinned dep). ~~Local CH virtio-sound stays in scope (CH-support-gated).~~ **Corrected 2026-07-10:** cloud-hypervisor is deleted outright (QC-15), so "CH-support-gated" no longer applies — see `docs/design/e12-9-10-libvirt-rescope.md` for the re-scoped local-audio path (QEMU/libvirt native `<audio type='pipewire'>`, feeding the already-shipped E12-16 PipeWire mixer). Clipboard + mesh-share bridges already done. E12-9 stays [>] on the local-audio remainder.
- **MOTION-TRANS-4 + MOTION-PERF-4 → [✗] WON'T-DO** — their acceptance targets the retired iced/Cosmic compositor; carrying the polish to the egui/Quasar shell would be net-new work, not completion (mirrors GUI-9).
- **ROUTER-6 → stays [!] DEFERRED-YAGNI** — single EdgeRouter; un-defer only at a 2nd appliance (migrating live prod DHCP/firewall state for zero current benefit is pure risk).
- **12.1 release: KEEP ACCUMULATING** — no cut yet; drain the DAR live tail + VDI bed first.
- **AUTHORIZED (2026-07-03): standing prod-SSH + XCP cloud create/delete + maintenance window** (DAR DevOps-rebuild live tail) + **stand up the live Quasar VDI test bed** (E12 VDI live legs).
- **DAR-19** (parked 2026-07-03) — Genesis-fresh control VM is STAGED + de-risked: mcnf-control (a68ab38b) live at 172.20.145.190, magic-mesh 11.3.1 + nebula installed (channel repo path fix: fedora-43-x86_64), both LHs' external-addr set. FINAL BLOCKER: the 11.3.1 enroll-token emits the OVERLAY endpoint (10.42.0.1:4242) not the public enroll endpoint (:4243), so a fresh non-overlay node's CSR can't reach the LH signer. Needs the mesh bootstrap-enroll path for a fresh box (cf. LH-JOIN-QNM-1) — operator/focused-mesh work.

## DAR genesis-fresh — STAGED, final enroll layer blocked (2026-07-03)
Control plane rebuild is de-risked to its last mile. State to resume:
- **Control VM:** `mcnf-control` UUID `a68ab38b-a9aa-8f97-ef36-705aead0e34a` on founder `.193`, live at `172.20.145.190` (ssh `mm@` with `id_ed25519`). `magic-mesh 11.3.1` + `nebula` installed. A `cidata` VDI (`d8103f1d`) supplies the seed (CONTROLVM-9 workaround — the proper fix is a control-plane golden delivering cidata, or the golden reading XenStore `vm-data/user-data`).
- **LHs:** both external-addr set to public IPs (`165.227.188.238:4242`, `104.131.64.207:4242`).
- **FINAL BLOCKER:** `mackesd enroll --token` on the fresh node published a CSR but the LH never signed ("no bundle in 30s"). The 11.3.1 `enroll-token` embeds `mesh:magic-mesh@10.42.0.1:4242` (overlay + nebula port), but a non-overlay bootstrap needs the LH's public IP at the **enroll port `:4243`** (as the original seed token `@167.71.247.150:4243` did). Need the fresh-box bootstrap-enroll path against the live 11.3.1 LHs — connects to worklist **LH-JOIN-QNM-1**.
- **Reversible:** destroy `a68ab38b` + the `cidata` VDI to clean up; LH external-addr changes are corrective (safe to keep).
- **Proper IaC follow-on:** bake a control-plane golden (magic-mesh RPM installed) so `tofu apply` produces an enroll-ready VM (DAR-34/49); the CONTROLVM-9 cidata-delivery fix belongs in the `control-vm` tofu root.

## Naming & consistency (operator decisions — from the 2026-07-10 platform review)

_These are genuine product/naming decisions, not coding-agent work. Recorded here from the docs-consistency drain so the sweep does not proceed on a guess. **No strings were mass-renamed.**_

- **NAMING-1: brand spelling "Quazar" (Z) vs "Quasar" (S) — ✅ RESOLVED 2026-07-17; sweep tracked by `WL-UX-004`** _(review `docs-consistency-2` / `shell-ux-9`)._
  - **Decision.** The operator confirmed **"Quazar"** as canonical for user-facing strings and governance/docs, honoring QBRAND lock **#9/#10**. **`magic-mesh` stays the package/repo/infra id** (the GNOME-vs-gnome-shell split, QBRAND #10), and internal identifiers / asset paths are not renamed unless separately justified.
  - **Current state.** The brand crate implements the decision (`crates/shared/mde-theme/src/brand/build.rs` `12 => "Quazar"`; `brand::logo::PRODUCT_NAME` is "MDE Quazar"). The active sweep has already corrected Browser dashboard, phone endpoint/notification identity, Console provenance, and Device Manager report/menu labels; remaining user-facing strings are tracked in `docs/platform/WORKLIST.md` under `WL-UX-004`.

- **NAMING-2: one VM vocabulary + panel-ownership badging — a follow-up cleanup** _(review `docs-consistency-8`)._ The docs now state the intended split (Cloud plane = **"instances"**, Fleet ▸ Datacenter = **"VMs"**; Nova-managed domains are read-only in Datacenter — see `docs/help/cloud-self-service.md`). The remaining work is UI-side and not done here: badge Nova-managed rows in the Datacenter panel and cross-link the two surfaces, and resolve the product-scope tension the rescope doc flags (`docs/design/e12-9-10-libvirt-rescope.md` §Current architecture / quasar-cloud.md **Q38**: two independent VM-creation paths writing the same `/var/lib/mde-vms` dir-pool). Needs an owner for the Q38 scope call.
