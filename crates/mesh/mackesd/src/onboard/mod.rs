//! OW-2 — the `mackesd onboard` engine core.
//!
//! Onboarding a node is a sequence of verbs (create a mesh, issue an invite,
//! enroll, wire mesh-DNS + the overlay network, self-check, provision the role's
//! services). Both front-ends drive that sequence: the egui first-run wizard and
//! the headless TUI (`mde-enroll` over SSH). They must share ONE engine, not
//! reimplement the steps twice — so every verb lives here as a pure core plus a
//! thin, headless shell, and the `mackesd onboard <verb>` dispatcher (in
//! `bin/mackesd.rs`) is the CLI face of the same functions the GUIs call
//! in-process.
//!
//! # This unit (OW-2) lands the dispatcher + the two home-less verbs
//! * [`self_test`] — a node self-diagnostic: KVM virtualization stack readiness
//!   (over [`crate::kvm::KVM_SERVICES`]), the mesh peer directory, and
//!   identity + CA presence, folded into a headless JSON/human report with a
//!   critical-fail exit code. **Extended (OW-10)** with the per-item live checks —
//!   overlay reachable, role daemons active (reused role→units model), CA-signed
//!   cert, lighthouse pingable — each a pure classification over a fact from an
//!   injectable probe seam that returns a typed gated `Unknown` when it can't run
//!   headless (never a faked pass, never a hard-fail on a gated probe).
//! * [`role_provision`] — apply a deployment role's systemd unit set: enable the
//!   units the role runs and mask the ones it does not, with the role→units set
//!   derived from the `mde_role` rank model (the same tiering
//!   [`crate::worker_role`] gates the in-process workers by).
//!
//! # Landed since OW-2: OW-3 [`mesh_create`] + OW-4 [`invite`] + OW-5 [`network`] + OW-6 [`mesh_dns`] + OW-7 [`spawn_lighthouse`]
//! * [`mesh_create`] (OW-3) founds a lone Workstation's mesh-of-one (mint CA +
//!   LAN-only overlay, offline) — a thin idempotent wrapper over the ENT-4
//!   [`crate::mesh_init`] bootstrap (reuse, not a reimplementation).
//! * [`invite`] (OW-4) mints a short-TTL, mesh-scoped join invite (an authenticated
//!   bearer recorded in [`crate::bearer_ledger`], a typeable code + a QR string) and
//!   exposes the pure redemption check the joiner pairs with the ledger.
//! * [`network`] (OW-5) brings up the primary LAN interface *before* the overlay:
//!   detect DHCP-vs-static (reusing [`crate::router_discovery`]'s default-gateway
//!   detection) and write the correct NetworkManager keyfile, so a fresh box reaches
//!   its LAN even on a static-only, no-DHCP network (the cloud-init NM-keyfile fix).
//! * [`mesh_dns`] (OW-6) publishes the mesh's name service: it folds the replicated
//!   peer roster ([`mackes_mesh_types::peers`]) into a `<host>.<mesh-id>` →
//!   overlay-IP zone and writes a managed `/etc/hosts` block, so operators reach
//!   nodes by name over the overlay without memorizing Nebula IPs (reusing the
//!   own-row directory, not a new sync).
//! * [`spawn_lighthouse`] (OW-7) promotes a lone Workstation's LAN-only mesh by
//!   standing up its first cloud lighthouse, push-enrolling it, and **migrating
//!   the CA** to it over #12's existing lighthouse-scoped-bearer CA-key delivery.
//!   This slice is the pure `plan_spawn` core + the injectable
//!   [`spawn_lighthouse::Provisioner`] apply seam (production `LiveProvisioner` is
//!   honestly integration-gated); the no-cloud-token case is a real `LanOnly` +
//!   retry branch, not a stub.
//! * [`first_desktop`] (OW-8, QC-15 cutover) plans + offers this Workstation's
//!   **first cloud-backed VM desktop**: it selects a VM image from the PLANES-22 image
//!   catalog, builds the VDI broker desktop-placement request, and emits the
//!   broker's `SessionRequest::Open` so the shell's Desktop surface renders it.
//!   This slice is the pure `plan_first_desktop` core + the injectable
//!   [`first_desktop::FirstDesktopApply`] seam (production `LiveFirstDesktop` is
//!   honestly integration-gated — needs a live Nova+Heat cloud + the Bus); the
//!   place/reconnect/no-image branches are all real outcomes, not stubs. The
//!   shell/DRM-boot half (E12-2/E12-3) is hardware-gated and lands in its own units.
//! * [`service_add`] (OW-11) adds a curated back-office service without blocking the
//!   working network (#20): **Music** provisions Navidrome on a media-lighthouse
//!   reading DO Spaces (reusing [`spawn_lighthouse`]'s `ProvisionSpec` + the peer
//!   roster's media tag to select/promote the target, #18/#19), **Files** is P2P
//!   `mde-files` Send-To with no VM (a real no-op outcome, never a spawn), and
//!   **Voice** registers to an external SIP provider (the password held in the secret
//!   store, never embedded). Pure `plan_service_add` core + the injectable
//!   [`service_add::ServiceApply`] seam (production `LiveServiceApply` is honestly
//!   integration-gated); no-lighthouse / no-SIP-account are real retryable outcomes.
//!
//! # Verbs still owned by the sibling OW units — deliberately NOT declared here (§7)
//! The remaining complex verbs land in their own units with real implementations;
//! this engine carries no stub / `todo!()` variant for any: `enroll` (its own OW unit).

pub mod first_desktop;
pub mod invite;
pub mod mesh_create;
pub mod mesh_dns;
pub mod network;
pub mod remote_push;
pub mod role_provision;
pub mod self_test;
pub mod service_add;
pub mod spawn_lighthouse;
