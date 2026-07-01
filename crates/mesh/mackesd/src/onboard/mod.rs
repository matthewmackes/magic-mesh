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
//!   critical-fail exit code.
//! * [`role_provision`] — apply a deployment role's systemd unit set: enable the
//!   units the role runs and mask the ones it does not, with the role→units set
//!   derived from the `mde_role` rank model (the same tiering
//!   [`crate::worker_role`] gates the in-process workers by).
//!
//! # Landed since OW-2: OW-3 [`mesh_create`] + OW-4 [`invite`]
//! * [`mesh_create`] (OW-3) founds a lone Workstation's mesh-of-one (mint CA +
//!   LAN-only overlay, offline) — a thin idempotent wrapper over the ENT-4
//!   [`crate::mesh_init`] bootstrap (reuse, not a reimplementation).
//! * [`invite`] (OW-4) mints a short-TTL, mesh-scoped join invite (an authenticated
//!   bearer recorded in [`crate::bearer_ledger`], a typeable code + a QR string) and
//!   exposes the pure redemption check the joiner (OW-5) pairs with the ledger.
//!
//! # Verbs still owned by the sibling OW units — deliberately NOT declared here (§7)
//! The remaining complex verbs land in their own units with real implementations;
//! this engine carries no stub / `todo!()` variant for any: `enroll` (OW-5) and
//! `mesh-dns` + `network` (OW-6).

pub mod invite;
pub mod mesh_create;
pub mod role_provision;
pub mod self_test;
