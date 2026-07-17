# Onboarding / First-Time Wizard — design lock

**Status:** LOCKED 2026-06-30 via a 50-question operator survey (`/plan`).
**Epic prefix:** `ONBOARD-WIZARD` (worklist) · **Identity:** E12 "Quazar" (12.0).
**One sentence:** one installer takes a bare box from first boot to a working mesh —
pick a role (Lighthouse / Workstation), create a new mesh **or** join an
existing one, and drive the whole path (CA bootstrap → optional lighthouse spawn →
optional back-office services) from a single role-branched wizard, `mackesd` as the
engine underneath.

> Authority note: this supersedes the older "Lighthouse = relay-only, no storage"
> blurb. Per Q19/Q21/Q41 the **Lighthouse role now also holds the media server +
> the CA/signer**. `AI_GOVERNANCE.md §5` (roles) must be updated to match.

> **⚠️ REVISION 2026-06-30 (post-survey design review — operator: "lock it").** A
> review after the survey changed several locks. **Where this block conflicts with
> the 50-lock table below, this block wins.**
>
> 1. **TWO roles only — Lighthouse and Workstation** (not three). The XCP-NG/Server
>    role is REMOVED. A **headless machine is a Workstation without a local display**
>    — it runs the daemon stack (`mackesd` + libvirt/QEMU-KVM + OVN + Podman) and
>    serves VMs/containers to the mesh, managed from a peer's Workbench; the
>    egui-DRM shell simply doesn't start with no seat. → **deletes lock #15**
>    (XCP-NG toolstack) and the Round-4/5 XCP-NG branches entirely.
> 2. **Hypervisor = Fedora + KVM (Option B), not XCP-ng.** XCP-ng is a day-2 "adopt
>    external capacity" action, never a role. The VM stack everywhere is
>    libvirt/QEMU-KVM + OVN + Podman; the older cloud-hypervisor/`mde-kvm` path is
>    superseded by `docs/design/quasar-cloud.md` and remains only as cutover/deletion
>    backlog until QC-15 removes it.
> 3. **Same stack on every machine; role = configuration, not a build.** One identical
>    image; `role=lighthouse|workstation` toggles systemd units (Option 1), evolving to
>    features-as-workloads (Option 2). → **supersedes lock #29**: role is now
>    **re-configurable** (flip a flag + attach a monitor), NOT fixed-at-install.
> 4. **The role-chooser is binary** (Lighthouse / Workstation). egui wizard where a
>    display exists; ratatui / `mackesd onboard` / push-from-a-peer for a headless
>    Workstation. (Locks #1–#4 about ISO/RPM split collapse: it's now ONE image, role
>    = config.)
> 5. **Management layer = mesh-native (#5)** — `mackesd` + libvirt/QEMU-KVM + OVN +
>    Podman over the existing etcd/Syncthing/Nebula, no center; Incus the
>    adopt-fallback; Nomad excluded (BUSL). See `docs/design/quasar-cloud.md`.
>
> The 50-lock table below is kept as the survey record — read it through this revision.

## The 50 locks

| # | Question | Lock |
|---|---|---|
| 1 | Installer delivery | **ISO** for Workstation + headless/XCP-NG (role chosen at install time); **RPM on Fedora 43** for Lighthouses |
| 2 | Wizard UI | **egui** on Workstation; **ratatui TUI** (mde-enroll-style) on headless XCP-NG/Lighthouse |
| 3 | First-run trigger | No role pinned → **role-chooser autostarts** → launches the role's wizard |
| 4 | "Same installer" | **One image, role chosen at install/boot time** |
| 5 | Workstation-first rule | **Hard-refuse**: "Create New Mesh" only exists on a Workstation; servers can only Join |
| 6 | Mesh CA origin | **Founding Workstation is the bootstrap CA** (mints CA + mesh id, signs joiners) |
| 7 | Join method | **QR / short code** shown by any enrolled node |
| 8 | Lighthouse-less start | **Yes** — LAN-only until a lighthouse is spawned later |
| 9 | Invite contents | **Founder reach-info + short-TTL join token**; *any* member can issue an invite |
| 10 | Lighthouse target | **Operator chooses cloud OR local** at spawn time |
| 11 | Spawn trigger | A deliberate **"Spawn Lighthouse" action in the Workstation Workbench** (post-create) |
| 12 | Cloud credential | Operator enters the token once → **stored in the mesh secret store** (age/etcd) |
| 13 | Lighthouse enroll | Wizard **SSHes in post-boot and push-provisions** the droplet (RPM + enroll) |
| 14 | Lighthouse HA | **Offer to spawn 2** (redundant pair) at first lighthouse setup |
| 15 | XCP-NG onboarding | **Provision the 16-service toolstack + join as a static Nebula member** (XCP-6: no mackesd on dom0) |
| 16 | Workstation onboarding | **egui shell + VDI stack + offer to create the first VM desktop** |
| 17 | Service catalog | Curated **golden-image VMs: Music, Files, Voice/SIP** |
| 18 | Music service | **Always spawn a new mesh-native Music service** |
| 19 | Service placement | **Lighthouses hold the media servers** |
| 20 | Service timing | **Separate post-onboarding "Services" flow** (never blocks the working network) |
| 21 | CA custody long-term | **Migrates to the Lighthouse** (always-on signer) once one exists |
| 22 | "Done" criterion | **Overlay up + ≥1 Workstation + off-LAN reach + self-test green** |
| 23 | Reinstall/recovery | **Re-enroll fresh; operator revokes the old identity** (no key backup) |
| 24 | Success view | **The live mesh map (`mde-mesh-view`)** is the confirmation |
| 25 | Role recommendation | **No hardware detection** — operator picks the role blind |
| 26 | Network setup | Wizard **configures the NIC (NM-keyfile fix) + detects LAN** (DHCP vs static) |
| 27 | Storage per role | **WS → `~/Local`; XCP-NG → SR; Lighthouse → a media volume** |
| 28 | Second Workstation | Joins via QR/invite; **own shell + VDI + per-peer session roaming** |
| 29 | Role upgrade | **Roles fixed at install; upgrade = reinstall** (no in-place rank bump) |
| 30 | Interrupt/resume | **Restart fully** — no saved progress, no idempotency guarantee (so steps must be fast+reliable) |
| 31 | Mesh naming | **Auto-generated mesh ID + optional friendly label** |
| 32 | Node revocation | **Passive** — delete the node, its **short-lived cert** simply expires (no CRL) |
| 33 | No-cloud lighthouse | **Stay LAN-only + retry cloud later** (don't block) |
| 34 | Service-VM enroll | **Behind its host's cert** (the host represents its service VMs; not individual members) |
| 35 | Media library storage | **DO Spaces** (S3 object store) |
| 36 | Voice service | **Connect to an existing SIP provider** (no PBX VM) |
| 37 | Files service | **Peer-to-peer only** — the shipped `mde-files` Send-To over the Bus; no central VM |
| 38 | Updates | **bootc image-swap (Workstation) / dnf (headless)**; mesh identity + state persist |
| 39 | Offline / airgapped | **Yes — LAN-only onboarding is first-class**; cloud features degrade gracefully |
| 40 | Day-2 additions | **From the Workbench** — invite / spawn-lighthouse / add-service actions, anytime |
| 41 | Music server software | **Navidrome (Subsonic API) backed by DO Spaces**; `mde-music-egui` is the client |
| 42 | Chooser → wizard handoff | **Role-chooser captures role + new/join**, launches the matching flow |
| 43 | Disclaimer | **Show the `mde-disclaimer` first, require acknowledgement** |
| 44 | mackesd's role | **Wizard is a front-end; `mackesd` is the engine** (gains onboard / session-broker / vm-lifecycle workers) |
| 45 | Mesh DNS | **`<host>.<mesh>` resolvable over the overlay** (served/synced by the CA holder) |
| 46 | Mesh-of-one | **Valid** — a single Workstation that created its mesh is a complete working network |
| 47 | Self-test contents | **Overlay reachable + role daemons active + CA-signed + (if present) lighthouse pingable** |
| 48 | Invite security | **Short TTL (minutes), mesh-scoped, shown as QR + typeable short code** |
| 49 | Bootc tie-in | *(survey blank → default)* Workstation wizard **baked into the E12-13 bootc image**; the ISO lays that image down |
| 50 | Tenancy | **One box, one mesh** (reinstall to switch) |

## Resulting architecture

### The single flow (role-branched, `mackesd`-driven)
```
boot → role-chooser (DISCLAIMER ack, §43) ─┬─ Lighthouse ─┐
   (no role pinned, §3)                    ├─ XCP-NG ──────┤  role pinned (§6 upgrade-fixed, §29)
   role + new/join captured here (§42) ────┴─ Workstation ─┘
                                                   │
        ┌──────────────────────────────────────────┴───────────────┐
   CREATE (WS only, §5)                                        JOIN (any role)
   • WS mints CA + mesh-id (§6,§31)                            • scan QR / type code (§7,§48)
   • configures NIC/LAN (§26)                                  • CSR → bundle (token, §9)
   • LAN-only overlay up (§8)  ── mesh-of-one is DONE (§46)    • role stack provisioned (§15,§16)
   • [Workbench] Spawn Lighthouse (§11) ──► cloud|local (§10)
        • push-provision over SSH (§13), offer a PAIR (§14)
        • CA migrates to the lighthouse (§21)
                                                   │
                              self-test (§47) ──► live mesh map (§24)
                                                   │
                       [post-onboarding] Services flow (§20) — day-2 from Workbench (§40)
                          • Music: Navidrome on a Lighthouse, library on DO Spaces (§18,§19,§35,§41)
                          • Files: P2P mde-files Send-To, no VM (§37)
                          • Voice: configure an external SIP provider (§36)
```

### Roles (redefined by this survey)
- **Lighthouse** — relay + control plane **+ media server (Navidrome→DO Spaces) + the CA/signer** once migrated. RPM on Fedora 43, cloud or local. Holds a media volume (§27). *(This is the big change vs the old "relay-only".)*
- **XCP-NG** — the 16-service toolstack + a static Nebula member; serves VM desktops + hosts the back-office Files/service VMs behind its own cert (§34). Headless TUI onboarding.
- **Workstation** — the Quazar egui shell + VDI; the **only role that can found a mesh**; the bootstrap CA until a lighthouse takes over; per-peer session roaming. ISO/bootc.

### `mackesd` engine surface (new workers/verbs the wizard drives)
`onboard` · `mesh-create` (CA + id) · `invite-issue` (QR + short code, short-TTL) ·
`enroll` (CSR→bundle, reuse ONBOARD-3) · `spawn-lighthouse` (cloud via zone1-do IaC /
local via libvirt/QEMU-KVM; push-provision; pair) · `ca-migrate` ·
`role-provision` (per-role stack) · `service-add` (Navidrome/Files/Voice) ·
`mesh-dns` (`<host>.<mesh>`) · `self-test` · `revoke` (passive/expiry).

## Acceptance (each runtime-observable, §7)
- A bare Workstation ISO install reaches the **role-chooser → Create New Mesh →** a
  working LAN-only mesh-of-one with the egui shell, **no internet** (§39,§46).
- A second box **scans the QR / types the code** and joins the same mesh, reachable
  by `<host>.<mesh>` over the overlay (§7,§45).
- From the Workstation Workbench, **Spawn Lighthouse** stands up a cloud (or local)
  lighthouse **pair**, push-provisioned, and the **CA migrates** to it; off-LAN
  reach now works (§11,§13,§14,§21).
- The **self-test** renders green per-item, then the **mesh map** shows the nodes
  (§47,§24).
- The post-onboarding **Services** flow stands up **Navidrome on a lighthouse reading
  DO Spaces**, and `mde-music-egui` lists+plays from it (§18,§19,§35,§41).
- An XCP-NG box onboards over **TUI** to the toolstack + static Nebula member and
  serves a VM desktop (§2,§15).
- A reinstalled box **re-enrolls fresh**; the old cert is revoked/expires (§23,§32).

## Risks / watch
- **Cloud coupling** (DO droplet + DO Spaces) vs the §0 no-fixed-center + airgapped
  reality — mitigated by LAN-first (§39) + local-lighthouse fallback (§33), but media
  on DO Spaces means **no offline music** unless a local cache/mirror is added (open).
- **CA migration to a cloud lighthouse** centralizes signing on an off-prem box —
  acceptable per operator, but a lighthouse loss = no new enrollments until recovery.
- **No resume (§30)** demands each step be fast + reliable, or a mid-spawn failure
  forces a full restart; the push-provision (§13) is the riskiest long step.
- **Lighthouse role overload** (relay+control+media+CA) raises its blast radius;
  the §8 envelope for it must be revisited.

## Out of scope (v1)
- In-place role upgrades (§29 — reinstall instead).
- Onboarding resume/idempotency (§30).
- Active CRL revocation (§32 — passive expiry only).
- Multi-mesh tenancy (§50).
- Hardware auto-role-detection (§25).
- A Voice PBX VM (§36 — external SIP only).
