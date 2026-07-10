//! VOIP-GW-2 — the typed Vitelity API client.
//!
//! A Vitelity-specific (v1, no provider abstraction) HTTP-API client
//! behind an **injectable seam** so the `voice_provision` worker
//! (VOIP-GW-3) can drive it headless with a [`FakeVitelityClient`] and
//! run the real path only where the master API key + network are
//! present ([`LiveVitelityClient`]). It covers the operations the
//! per-node-SIP design (`docs/design/voice-vitelity-per-node-sip.md`,
//! locks 11 + 14) needs:
//!
//! - sub-account **create / list / get** (each node's inbound identity),
//! - DID **list / route** — route an *existing* master-account DID to a
//!   sub-account (**no new-DID provisioning**, lock 11),
//! - per-sub-account **failover / voicemail** config (lock 10).
//!
//! ## Shape (the typed, integration-gated client pattern)
//!
//! - The request-building (URL + query assembly) and response-parsing
//!   (Vitelity's XML → typed structs) logic lives in [`request`] and
//!   [`response`] as **pure functions**, unit-tested against fixture
//!   strings — no network in tests.
//! - [`LiveVitelityClient`] holds the master API key as a *parameter*
//!   (never a global, never in process args) and — until VOIP-GW-3
//!   wires a real HTTP transport — returns a typed
//!   [`VitelityError::IntegrationGated`] naming exactly what the live
//!   call needs (the master API key + network reachability to the
//!   Vitelity endpoint). That is a complete, honest behavior, **never a
//!   fake success** (§7).
//! - [`FakeVitelityClient`] is an in-memory double for tests /
//!   headless reconcile exercising.
//!
//! ## Secrets
//!
//! Credentials ([`VitelityCredentials`]) live in a dedicated type whose
//! `Debug` is redacted and whose secret is zeroized on drop. The pure
//! request builders in [`request`] produce only the **non-secret**
//! business params — the login + API key are injected by the live
//! transport at send time and are never carried in a [`VitelityRequest`]
//! (so a request can be logged safely).

pub mod fake;
pub mod live;
pub mod model;
pub mod request;
pub mod response;

pub use fake::FakeVitelityClient;
pub use live::LiveVitelityClient;
pub use model::{
    CreateSubAccount, Did, DidRouting, FailoverPolicy, SubAccount, SubAccountCredentials,
    SubAccountSummary, VoicemailConfig,
};

/// A typed failure from the [`VitelityClient`] seam.
///
/// `IntegrationGated` is the live-path signal that the operation is not
/// runnable in this build/environment yet — it names the operation and
/// exactly what it needs (the master API key + network). It is a real
/// typed error returned by a real method, never a stub (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VitelityError {
    /// The live path cannot run here yet: it needs the master API key +
    /// live network reachability to the Vitelity endpoint. Names the
    /// operation and what is missing.
    IntegrationGated {
        /// The seam operation (`create-subaccount`, `route-did`, …).
        op: &'static str,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// The Vitelity endpoint returned a non-success status. Carries the
    /// operation and Vitelity's own error text (parsed from the XML).
    Api {
        /// The seam operation that failed.
        op: &'static str,
        /// Vitelity's error message.
        message: String,
    },
    /// A response could not be parsed into the expected typed shape.
    Parse {
        /// The seam operation whose response failed to parse.
        op: &'static str,
        /// What went wrong parsing the response body.
        detail: String,
    },
    /// A concrete transport / runtime failure reaching Vitelity.
    Transport {
        /// The seam operation that failed.
        op: &'static str,
        /// The failure detail.
        detail: String,
    },
}

impl std::fmt::Display for VitelityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { op, reason } => {
                write!(f, "{op}: integration-gated — {reason}")
            }
            Self::Api { op, message } => write!(f, "{op}: vitelity error — {message}"),
            Self::Parse { op, detail } => write!(f, "{op}: parse error — {detail}"),
            Self::Transport { op, detail } => write!(f, "{op}: transport error — {detail}"),
        }
    }
}

impl std::error::Error for VitelityError {}

/// The injectable Vitelity operations seam.
///
/// Production is [`LiveVitelityClient`]; tests + headless reconcile use
/// [`FakeVitelityClient`]. Every method is Vitelity-specific (v1) and
/// maps to one Vitelity API command; the request/response translation is
/// the pure [`request`] / [`response`] folds.
pub trait VitelityClient {
    /// Create a receive-only sub-account (a node's inbound SIP identity).
    /// Returns the created sub-account plus its freshly minted SIP
    /// credentials — the caller (VOIP-GW-3) seals those to the node.
    ///
    /// # Errors
    /// [`VitelityError::IntegrationGated`] on the live path until a real
    /// transport is wired; otherwise `Api` / `Parse` / `Transport`.
    fn create_sub_account(
        &self,
        req: &CreateSubAccount,
    ) -> Result<(SubAccount, SubAccountCredentials), VitelityError>;

    /// List the master account's existing sub-accounts (one per node).
    ///
    /// # Errors
    /// [`VitelityError`] as above.
    fn list_sub_accounts(&self) -> Result<Vec<SubAccountSummary>, VitelityError>;

    /// Fetch one sub-account's detail by its Vitelity username.
    ///
    /// # Errors
    /// [`VitelityError`] as above.
    fn get_sub_account(&self, username: &str) -> Result<SubAccount, VitelityError>;

    /// List the master account's **existing** DIDs and their current
    /// routing (lock 11 — no new-DID provisioning).
    ///
    /// # Errors
    /// [`VitelityError`] as above.
    fn list_dids(&self) -> Result<Vec<Did>, VitelityError>;

    /// Route an existing DID to a sub-account (or back to the main
    /// account). Does **not** provision a new DID.
    ///
    /// # Errors
    /// [`VitelityError`] as above.
    fn route_did(&self, did: &str, routing: &DidRouting) -> Result<(), VitelityError>;

    /// Set a sub-account's offline-inbound failover policy (lock 10).
    ///
    /// # Errors
    /// [`VitelityError`] as above.
    fn configure_failover(
        &self,
        username: &str,
        policy: &FailoverPolicy,
    ) -> Result<(), VitelityError>;

    /// Configure a sub-account's voicemail.
    ///
    /// # Errors
    /// [`VitelityError`] as above.
    fn configure_voicemail(
        &self,
        username: &str,
        vm: &VoicemailConfig,
    ) -> Result<(), VitelityError>;
}
