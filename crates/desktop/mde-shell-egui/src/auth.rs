//! CHOOSER-6 — **desktop connect authentication** (design `docs/design/desktop-chooser.md`,
//! lock 8; WORKLIST CHOOSER-6).
//!
//! Every desktop the Chooser ([`crate::chooser`]) connects authenticates one of
//! two ways, and this module is the typed model + resolution fold for both:
//!
//!   * **Mesh-brokered source** (a peer seat / peer VM / local VM) authenticates
//!     with **this node's mesh identity — SSO, no credential prompt**. The mesh
//!     identity is [`crate::discovery::local_peer`] (the node hostname the mesh
//!     keys nodes by), the SAME value the broker `Open` request already carries as
//!     its `client_peer` ([`crate::discovery::publish_open`]). So an SSO connect
//!     never touches the secret store and never prompts (§6 — reuse the mesh
//!     identity, don't mint a second login).
//!   * **External RDP/VNC/Spice endpoint** (an mDNS / manual source, off the mesh
//!     broker) authenticates with a **credential sealed in the secret store**,
//!     resolved once then remembered. The credential feeds the protocol config's
//!     password field (`mde_vdi_rdp::RdpConfig::password`, `…vnc::VncConfig::with_password`,
//!     `…spice::SpiceConfig::with_password` — each documents it is "sourced from
//!     the sealed cred vault") on the gated live transport (E12-4).
//!
//! ## §6 — reuse FILEMGR-6's seal, don't re-roll crypto, don't link the daemon
//!
//! FILEMGR-6 sealed the shared mesh SSH key with the **one audited envelope**
//! (`mackesd::ca::backup::seal_bytes` — Argon2id + XChaCha20-Poly1305) via the
//! mesh secret store (`mackesd::ipc::secret_store::SecretStore`, age + etcd, with
//! a local-AEAD fallback). That store + its seal live INSIDE the `mackesd` daemon
//! crate, gated behind its heavy `async-services` feature (tokio / zbus / etcd),
//! which the desktop-shell tier must never link (§6 — the shell leans inward on
//! `mde-bus` only; every other surface mirrors the Bus JSON rather than the daemon
//! type). So this unit does NOT re-implement the envelope and does NOT link the
//! daemon: it drives an injectable [`CredentialStore`] **seam** (the exact shape
//! of [`crate::chooser::DesktopSourcesClient`] and the FILEMGR-9 mesh-mount seam),
//! and the store-key derivation ([`derive_store_ref`]) mirrors the secret store's
//! own `creds_ref_for` (`vpn/…`) / `xcp_creds_ref` (`xcp/…`) namespacing — a
//! sanitized, prefixed, pure + stable `desktop/…` key.
//!
//! ## §7 — honest states, no fake success, no plaintext credential
//!
//! The **resolution fold is real + fully unit-tested** (SSO-vs-sealed decision,
//! prompt-once-then-remember, the seal→store→read round-trip against a fake store
//! that records seals). The plaintext credential is NEVER written to disk here and
//! NEVER reaches a log: [`Secret`] redacts through `Debug` and has no `Display`, so
//! a `DesktopAuth` / `ConnectRequest` can be `{:?}`-logged safely. The **live seal
//! into the mesh store** is the honest-gated leg (the audited seal + the mesh age
//! identity are mesh-side; a desktop-tier sealing path is brokered by a mesh
//! worker, not shipped in this unit) — exactly the discipline CHOOSER-5's live
//! connect and E12-4's live transport use. A gated seal still lets the just-entered
//! credential drive THIS session in-memory; it simply isn't persisted, and the
//! Chooser says so plainly rather than faking "remembered".

use crate::vdi::VdiProtocol;

/// A secret string (a credential password / VNC RFB secret / Spice ticket) that
/// never leaks through `Debug`, `Display`, or a log line — the CHOOSER-6
/// "never a plaintext credential … in logs" invariant. The plaintext is reachable
/// only through [`Secret::expose`], the single sanctioned exit the gated live
/// transport (E12-4) feeds into the protocol config's password field.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    /// Wrap a plaintext secret.
    pub(crate) fn new(plaintext: impl Into<String>) -> Self {
        Self(plaintext.into())
    }

    /// The plaintext — the ONE sanctioned exit, called only where the secret must
    /// be handed to the protocol client (never to a log). The live transport that
    /// feeds it into the protocol config's password field is the gated E12-4 leg,
    /// so until that's wired only the tests exercise this exit (§7-gated, like
    /// `vdi::Session`).
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the plaintext exit is fed to the protocol client by the gated live transport (E12-4); until it is wired, only tests call it"
        )
    )]
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    /// Redact — a `Debug` of any structure carrying a `Secret` prints a placeholder
    /// so a credential can never land in a log/trace line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}
// Deliberately NO `Display` impl: a `Secret` cannot be `{}`-formatted at all, so it
// can't be accidentally written into a formatted log/UI string.

/// An external endpoint's credential. `username` is empty for the protocols that
/// authenticate on a bare secret (classic VNC RFB, an anonymous Spice ticket); the
/// [`Secret`] carries the password and is redacted from `Debug`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Credential {
    /// The login user (may be empty for VNC/Spice bare-secret auth).
    pub(crate) username: String,
    /// The password / RFB secret / Spice ticket — redacted from logs.
    pub(crate) secret: Secret,
}

impl Credential {
    /// Assemble a credential from a username + plaintext secret.
    pub(crate) fn new(username: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            secret: Secret::new(secret),
        }
    }
}

/// How a desktop connect authenticates (design lock 8) — the resolved auth the
/// [`crate::vdi::ConnectRequest`] carries.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum DesktopAuth {
    /// A mesh-brokered source: THIS node's mesh identity authenticates — SSO, no
    /// credential prompt. `node` is [`crate::discovery::local_peer`] (the same
    /// mesh identity the broker `Open` carries as `client_peer`).
    MeshIdentity {
        /// This node's mesh identity (hostname).
        node: String,
    },
    /// An external endpoint: a credential sealed in the secret store under
    /// `store_ref`, resolved once then remembered.
    Sealed {
        /// The `desktop/…` secret-store key the credential is sealed under.
        store_ref: String,
        /// The resolved credential (fed to the protocol client on connect).
        credential: Credential,
    },
}

impl DesktopAuth {
    /// The mesh-identity SSO auth for a mesh-brokered source.
    pub(crate) fn mesh_identity(node: impl Into<String>) -> Self {
        Self::MeshIdentity { node: node.into() }
    }

    /// A short, log-safe summary of how the connect authenticates — for the honest
    /// connect note / caption (§7). NEVER includes the secret.
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::MeshIdentity { node } => format!("mesh identity ({node}) — SSO"),
            Self::Sealed { credential, .. } if credential.username.is_empty() => {
                "sealed credential".to_string()
            }
            Self::Sealed { credential, .. } => {
                format!("sealed credential ({})", credential.username)
            }
        }
    }
}

/// Derive the stable secret-store key an external endpoint's credential seals
/// under, namespaced `desktop/<host>/<protocol>`.
///
/// The `desktop/` prefix namespaces these creds away from the datacenter secrets
/// (`vpn/…`, `xcp/…`, `copilot/…`) sharing the store — the SAME discipline as
/// `mackesd::ipc::secret_store::{creds_ref_for, xcp_creds_ref}`. `host` is
/// sanitized to the address charset (alnum, `.`, `-`, `:`, `_`) so a stray
/// separator can't widen the key namespace, exactly as `xcp_creds_ref` sanitizes.
/// Keying by protocol as well as host lets one host hold distinct RDP vs VNC vs
/// Spice creds. Pure + stable: this string IS the store key, so a change orphans
/// the sealed credential.
pub(crate) fn derive_store_ref(host: &str, protocol: VdiProtocol) -> String {
    let safe: String = host
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        .collect();
    format!("desktop/{safe}/{}", protocol.label().to_ascii_lowercase())
}

/// The one-time credential prompt for an external endpoint with no sealed
/// credential yet: the live text-edit buffers the Chooser's picker binds, plus the
/// store key the entered credential will seal under. The `password` buffer is
/// bound to a masked field and moves into a redacted [`Secret`] on
/// [`CredentialPrompt::to_credential`]; it is never logged (the custom `Debug`
/// redacts it) and never written in plaintext.
pub(crate) struct CredentialPrompt {
    /// The `desktop/…` store key the entered credential seals under.
    pub(crate) store_ref: String,
    /// The username edit buffer (may stay empty for VNC/Spice).
    pub(crate) username: String,
    /// The password edit buffer — masked in the UI, sealed on confirm, never logged.
    pub(crate) password: String,
}

impl std::fmt::Debug for CredentialPrompt {
    /// Redact the password buffer so a logged draft can't leak the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialPrompt")
            .field("store_ref", &self.store_ref)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl CredentialPrompt {
    /// A fresh, empty prompt keyed to `store_ref`.
    pub(crate) fn new(store_ref: impl Into<String>) -> Self {
        Self {
            store_ref: store_ref.into(),
            username: String::new(),
            password: String::new(),
        }
    }

    /// Fold the entered buffers into a [`Credential`] (the plaintext password moves
    /// into a redacted [`Secret`]).
    pub(crate) fn to_credential(&self) -> Credential {
        Credential::new(self.username.clone(), self.password.clone())
    }
}

/// Where a connect stands on auth after resolution — either ready to connect
/// (SSO, or a sealed credential read back) or waiting on a one-time prompt.
#[derive(Debug)]
pub(crate) enum AuthStage {
    /// Ready: mesh-identity SSO, or a remembered sealed credential.
    Ready(DesktopAuth),
    /// An external endpoint with no sealed credential — prompt once, then seal.
    Prompt(CredentialPrompt),
}

/// The outcome of trying to seal a just-entered credential — distinguished so the
/// Chooser reports honestly (§7) and never fakes "remembered".
///
/// `Gated` is the only variant the honest-gated production store
/// ([`MeshCredentialStore`]) returns today; `Sealed` / `Failed` are produced by the
/// live/real mesh-side store (and by the round-trip tests), so a non-test build
/// constructs only `Gated` — the same gated shape as `vdi::Session`.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Sealed/Failed are produced by the live mesh-side credential store; the honest-gated production store returns only Gated until that seal path is wired"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SealOutcome {
    /// The credential was sealed at rest (persisted) — genuinely remembered.
    Sealed,
    /// Sealing is honest-gated on this node (the audited seal + mesh age identity
    /// are mesh-side); the credential drives THIS session in-memory but isn't
    /// persisted. Carries the operator-readable reason.
    Gated(String),
    /// The store faulted (crypto / I/O) — surfaced, never swallowed as success.
    Failed(String),
}

/// The sealed-credential store seam. Injectable so the resolution fold is
/// unit-tested headless with a fake while production drives the honest-gated
/// mesh-side store ([`MeshCredentialStore`]) — the [`crate::chooser::DesktopSourcesClient`]
/// / FILEMGR-9 mesh-mount pattern.
pub(crate) trait CredentialStore {
    /// The sealed credential under `store_ref`, decrypted; `Ok(None)` when nothing
    /// is sealed yet (→ prompt once). `Err` on a real store fault — never a fake
    /// `None` that would hide a broken store as "not stored".
    fn get(&self, store_ref: &str) -> Result<Option<Credential>, String>;

    /// Seal `credential` under `store_ref` (encrypt at rest — reusing the mesh
    /// secret store, never plaintext). Returns a typed [`SealOutcome`] so a gated
    /// store is honest rather than a faked success.
    fn seal(&self, store_ref: &str, credential: &Credential) -> SealOutcome;
}

/// The production credential store — honest-gated (§7).
///
/// The audited seal (`mackesd::ca::backup::seal_bytes`) and the mesh age identity
/// that keys it live in the `mackesd` daemon (behind `async-services`), which the
/// desktop-shell tier must not link (§6). So a desktop-tier seal path is brokered
/// mesh-side by a secret-store worker — not shipped in this unit — and is gated
/// here: [`Self::get`] honestly reports nothing sealed on the desktop tier yet,
/// and [`Self::seal`] returns [`SealOutcome::Gated`] rather than faking
/// persistence. The credential the operator enters still drives the (itself gated,
/// E12-4) live connect in-memory; nothing plaintext is written.
pub(crate) struct MeshCredentialStore;

/// The honest gate message — named once so the note and any log agree.
const GATED_REASON: &str =
    "sealing external desktop credentials is brokered by the mesh secret store \
     (FILEMGR-6) mesh-side; the desktop-tier seal path is gated";

impl CredentialStore for MeshCredentialStore {
    fn get(&self, _store_ref: &str) -> Result<Option<Credential>, String> {
        // Nothing is sealed on the desktop tier yet (the sealed vault is mesh-side,
        // reached by the gated worker) — an honest "not stored", never a fault.
        Ok(None)
    }

    fn seal(&self, _store_ref: &str, _credential: &Credential) -> SealOutcome {
        SealOutcome::Gated(GATED_REASON.to_string())
    }
}

/// Resolve a source's auth at activate time (the SSO-vs-sealed decision).
///
/// A mesh-brokered source resolves to mesh-identity SSO WITHOUT touching the store
/// (no prompt, no read). An external endpoint derives its [`derive_store_ref`] key
/// and reads the store: a sealed credential resolves ready (remembered), an absent
/// one asks for a one-time prompt. A store fault surfaces as `Err` (never a silent
/// prompt that would hide a broken store).
pub(crate) fn resolve(
    is_mesh_brokered: bool,
    node: &str,
    host: &str,
    protocol: VdiProtocol,
    store: &dyn CredentialStore,
) -> Result<AuthStage, String> {
    if is_mesh_brokered {
        return Ok(AuthStage::Ready(DesktopAuth::mesh_identity(node)));
    }
    let store_ref = derive_store_ref(host, protocol);
    match store.get(&store_ref)? {
        Some(credential) => Ok(AuthStage::Ready(DesktopAuth::Sealed {
            store_ref,
            credential,
        })),
        None => Ok(AuthStage::Prompt(CredentialPrompt::new(store_ref))),
    }
}

/// Seal the one-time credential the operator entered for an external endpoint,
/// then hand back the resolved [`DesktopAuth::Sealed`] — used for THIS connect
/// in-memory regardless of the seal outcome, so a gated store still connects (the
/// entered secret is never dropped and never written in plaintext). The
/// [`SealOutcome`] tells the caller whether it was genuinely remembered.
pub(crate) fn remember(
    store: &dyn CredentialStore,
    prompt: &CredentialPrompt,
) -> (DesktopAuth, SealOutcome) {
    let credential = prompt.to_credential();
    let outcome = store.seal(&prompt.store_ref, &credential);
    (
        DesktopAuth::Sealed {
            store_ref: prompt.store_ref.clone(),
            credential,
        },
        outcome,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// An in-memory [`CredentialStore`] that does a REAL seal→store→read round-trip
    /// and RECORDS every seal so the tests can assert "sealed exactly once" and that
    /// no plaintext leaks. `seal_gated` flips it to the honest-gated behaviour.
    #[derive(Default)]
    struct FakeStore {
        sealed: RefCell<Vec<(String, Credential)>>,
        gated: bool,
    }

    impl FakeStore {
        fn gated() -> Self {
            Self {
                sealed: RefCell::new(Vec::new()),
                gated: true,
            }
        }

        /// How many seals have been recorded (the "prompt once" assertion).
        fn seal_count(&self) -> usize {
            self.sealed.borrow().len()
        }
    }

    impl CredentialStore for FakeStore {
        fn get(&self, store_ref: &str) -> Result<Option<Credential>, String> {
            Ok(self
                .sealed
                .borrow()
                .iter()
                .rev()
                .find(|(r, _)| r == store_ref)
                .map(|(_, c)| c.clone()))
        }

        fn seal(&self, store_ref: &str, credential: &Credential) -> SealOutcome {
            if self.gated {
                return SealOutcome::Gated("test gate".to_string());
            }
            self.sealed
                .borrow_mut()
                .push((store_ref.to_string(), credential.clone()));
            SealOutcome::Sealed
        }
    }

    /// A store that must NEVER be touched — proves the SSO path resolves without a
    /// read or a seal.
    struct ForbiddenStore;
    impl CredentialStore for ForbiddenStore {
        fn get(&self, _store_ref: &str) -> Result<Option<Credential>, String> {
            unreachable!("SSO must not read the credential store")
        }
        fn seal(&self, _store_ref: &str, _credential: &Credential) -> SealOutcome {
            unreachable!("SSO must not seal a credential")
        }
    }

    // ── the store-key derivation (mirrors the secret_store xcp_creds_ref tests) ──

    #[test]
    fn derive_store_ref_namespaces_under_desktop_and_is_stable_and_sanitized() {
        // An mDNS host:port keys the credential under desktop/<host>/<proto>…
        assert_eq!(
            derive_store_ref("192.168.1.60:3389", VdiProtocol::Rdp),
            "desktop/192.168.1.60:3389/rdp"
        );
        // …the protocol lower-cases so one host holds distinct RDP vs VNC creds…
        assert_eq!(
            derive_store_ref("10.0.0.5", VdiProtocol::Vnc),
            "desktop/10.0.0.5/vnc"
        );
        assert_eq!(
            derive_store_ref("10.0.0.5", VdiProtocol::Spice),
            "desktop/10.0.0.5/spice"
        );
        // …whitespace is trimmed and stray separators that could widen the key
        // namespace (a `/`, a space) are dropped — the xcp_creds_ref discipline.
        assert_eq!(
            derive_store_ref("  office pc/1 ", VdiProtocol::Rdp),
            "desktop/officepc1/rdp"
        );
    }

    // ── the SSO-vs-sealed decision ──

    #[test]
    fn a_mesh_brokered_source_resolves_to_sso_without_touching_the_store() {
        // The forbidden store panics on any access — reaching Ready proves SSO
        // never read or sealed (no credential prompt for a mesh peer).
        let stage = resolve(
            true,
            "this-node",
            "10.42.0.7",
            VdiProtocol::Rdp,
            &ForbiddenStore,
        )
        .expect("SSO resolves");
        let AuthStage::Ready(DesktopAuth::MeshIdentity { node }) = stage else {
            unreachable!("expected mesh-identity SSO")
        };
        assert_eq!(node, "this-node");
    }

    #[test]
    fn an_external_endpoint_with_no_sealed_credential_prompts_once() {
        let store = FakeStore::default();
        let stage = resolve(
            false,
            "unused",
            "192.168.1.60:3389",
            VdiProtocol::Rdp,
            &store,
        )
        .expect("resolves");
        let AuthStage::Prompt(prompt) = stage else {
            unreachable!("expected a one-time prompt")
        };
        assert_eq!(prompt.store_ref, "desktop/192.168.1.60:3389/rdp");
        assert!(prompt.username.is_empty() && prompt.password.is_empty());
        assert_eq!(store.seal_count(), 0, "resolving must not seal anything");
    }

    #[test]
    fn prompt_once_then_remember_seals_once_and_a_second_resolve_reads_it_back() {
        // The full CHOOSER-6 fold against a store that really round-trips.
        let store = FakeStore::default();
        let host = "192.168.1.60:3389";

        // First connect: no sealed cred → a prompt. The operator fills it once.
        let AuthStage::Prompt(mut prompt) =
            resolve(false, "n", host, VdiProtocol::Rdp, &store).expect("resolves")
        else {
            unreachable!("expected a prompt on first connect")
        };
        prompt.username = "administrator".to_string();
        prompt.password = "s3cr3t-pw".to_string();

        // Remember: seals exactly once and hands back the usable auth.
        let (auth, outcome) = remember(&store, &prompt);
        assert_eq!(outcome, SealOutcome::Sealed);
        assert_eq!(store.seal_count(), 1, "the credential is sealed once");
        let DesktopAuth::Sealed {
            store_ref,
            credential,
        } = &auth
        else {
            unreachable!("expected a sealed auth")
        };
        assert_eq!(store_ref, "desktop/192.168.1.60:3389/rdp");
        assert_eq!(credential.username, "administrator");
        assert_eq!(credential.secret.expose(), "s3cr3t-pw");

        // Second connect to the SAME endpoint: the sealed cred is read back —
        // remembered, NO prompt, and no additional seal.
        let stage = resolve(false, "n", host, VdiProtocol::Rdp, &store).expect("resolves");
        let AuthStage::Ready(DesktopAuth::Sealed { credential, .. }) = stage else {
            unreachable!("expected the remembered sealed cred")
        };
        assert_eq!(credential.username, "administrator");
        assert_eq!(credential.secret.expose(), "s3cr3t-pw");
        assert_eq!(store.seal_count(), 1, "a remembered connect never re-seals");
    }

    #[test]
    fn a_store_fault_surfaces_rather_than_silently_prompting() {
        struct FaultingStore;
        impl CredentialStore for FaultingStore {
            fn get(&self, _r: &str) -> Result<Option<Credential>, String> {
                Err("etcd unreachable".to_string())
            }
            fn seal(&self, _r: &str, _c: &Credential) -> SealOutcome {
                SealOutcome::Failed("etcd unreachable".to_string())
            }
        }
        let err = resolve(false, "n", "host", VdiProtocol::Rdp, &FaultingStore)
            .expect_err("a store fault is surfaced, not read as 'not stored'");
        assert!(err.contains("etcd"));
    }

    // ── the honest production gate (§7 — no fake success) ──

    #[test]
    fn the_production_store_is_honestly_gated_never_a_fake_success() {
        let store = MeshCredentialStore;
        // Honestly nothing sealed on the desktop tier yet (not a fault).
        assert_eq!(store.get("desktop/host/rdp").expect("honest none"), None);
        // A seal is gated, not faked as persisted.
        let cred = Credential::new("u", "p");
        let SealOutcome::Gated(reason) = store.seal("desktop/host/rdp", &cred) else {
            unreachable!("expected an honest gate")
        };
        assert!(reason.contains("mesh-side"));
    }

    #[test]
    fn a_gated_seal_still_yields_a_usable_in_memory_credential() {
        // On the live fleet the seal is gated — the just-entered credential still
        // drives THIS session in-memory (never written), the outcome says gated.
        let store = FakeStore::gated();
        let mut prompt = CredentialPrompt::new("desktop/host/vnc");
        prompt.password = "rfb-secret".to_string();
        let (auth, outcome) = remember(&store, &prompt);
        assert!(matches!(outcome, SealOutcome::Gated(_)));
        assert_eq!(store.seal_count(), 0, "a gated seal persists nothing");
        let DesktopAuth::Sealed { credential, .. } = auth else {
            unreachable!("expected the in-memory sealed auth")
        };
        assert_eq!(credential.secret.expose(), "rfb-secret");
    }

    // ── the no-plaintext-in-logs invariant (§7 / security) ──

    #[test]
    fn a_secret_never_leaks_through_debug() {
        let secret = Secret::new("hunter2-super-secret");
        let shown = format!("{secret:?}");
        assert_eq!(shown, "Secret(<redacted>)");
        assert!(
            !shown.contains("hunter2"),
            "the secret leaked through Debug"
        );
        // …and the plaintext is still reachable through the one sanctioned exit.
        assert_eq!(secret.expose(), "hunter2-super-secret");
    }

    #[test]
    fn a_sealed_auth_and_prompt_redact_the_credential_in_debug() {
        let auth = DesktopAuth::Sealed {
            store_ref: "desktop/host/rdp".to_string(),
            credential: Credential::new("admin", "leak-me-if-you-can"),
        };
        let shown = format!("{auth:?}");
        assert!(
            !shown.contains("leak-me-if-you-can"),
            "the sealed credential leaked through Debug: {shown}"
        );
        assert!(shown.contains("<redacted>"));
        // The username is not the secret and stays visible for diagnostics.
        assert!(shown.contains("admin"));

        // The live prompt buffer redacts its password too.
        let mut prompt = CredentialPrompt::new("desktop/host/rdp");
        prompt.password = "also-leak-me".to_string();
        let shown = format!("{prompt:?}");
        assert!(
            !shown.contains("also-leak-me"),
            "the prompt leaked: {shown}"
        );
        assert!(shown.contains("<redacted>"));
    }

    #[test]
    fn the_auth_summary_is_log_safe_and_never_carries_the_secret() {
        let sso = DesktopAuth::mesh_identity("oak");
        assert_eq!(sso.summary(), "mesh identity (oak) — SSO");

        let sealed = DesktopAuth::Sealed {
            store_ref: "desktop/host/rdp".to_string(),
            credential: Credential::new("admin", "top-secret"),
        };
        let summary = sealed.summary();
        assert_eq!(summary, "sealed credential (admin)");
        assert!(!summary.contains("top-secret"), "summary leaked the secret");

        // A bare-secret (VNC/Spice) credential summarises without a username.
        let bare = DesktopAuth::Sealed {
            store_ref: "desktop/host/vnc".to_string(),
            credential: Credential::new("", "rfb"),
        };
        assert_eq!(bare.summary(), "sealed credential");
    }
}
