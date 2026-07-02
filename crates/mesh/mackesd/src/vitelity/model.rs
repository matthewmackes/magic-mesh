//! Typed request/response structs for the Vitelity client (VOIP-GW-2).
//!
//! Plain data — no I/O. The [`request`](super::request) folds build
//! Vitelity query params from these; the [`response`](super::response)
//! folds parse Vitelity's XML into them.

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A node's inbound Vitelity sub-account (its external SIP identity).
///
/// The username derives from the node hostname (design lock 3) so the
/// callable address is a stable `<username>@<realm>`; that derivation
/// lives in the VOIP-GW-3 worker, not here — this is the API record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAccount {
    /// Vitelity sub-account username (the SIP auth user).
    pub username: String,
    /// Free-text label Vitelity stores against the sub-account
    /// (the worker sets it to the node hostname / nickname).
    pub description: String,
    /// The SIP realm/domain the sub-account registers to, forming the
    /// callable `<username>@<realm>` address. Empty until Vitelity
    /// reports it.
    pub realm: String,
}

/// A sub-account's SIP secret, returned once at create time.
///
/// Separate from [`SubAccount`] so the secret is never carried in the
/// loggable record. Redacted `Debug`, zeroized on drop — the worker
/// age-seals it to the node (design lock 7) and drops it.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct SubAccountCredentials {
    /// SIP auth username (mirrors [`SubAccount::username`]).
    pub username: String,
    /// SIP auth password — the node-sealed secret.
    pub sip_password: String,
}

impl std::fmt::Debug for SubAccountCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAccountCredentials")
            .field("username", &self.username)
            .field("sip_password", &"<redacted>")
            .finish()
    }
}

/// A compact sub-account row from the list endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAccountSummary {
    /// Vitelity sub-account username.
    pub username: String,
    /// The label Vitelity stores against it.
    pub description: String,
}

/// The create-sub-account request (design lock 2 — auto-provision).
///
/// Receive-only: outbound stays on the shared trunk (lock 4), bridged
/// Vitelity-side, so no outbound-trunk fields here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSubAccount {
    /// The sub-account username to create (hostname-derived by the
    /// caller).
    pub username: String,
    /// The label to store (node hostname / nickname).
    pub description: String,
}

/// An existing master-account DID and where it currently routes.
///
/// Lock 11: the client only lists + routes existing DIDs; it never
/// provisions a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Did {
    /// The DID (E.164-ish digits as Vitelity returns them).
    pub number: String,
    /// Current routing target, or `None` if routed to the main account.
    pub routed_to: Option<String>,
}

/// Where to route an existing DID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DidRouting {
    /// Route inbound calls to this sub-account username.
    SubAccount(String),
    /// Route back to the master account's main line.
    MainAccount,
}

/// A sub-account's offline-inbound failover policy (design lock 10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailoverPolicy {
    /// Send unanswered/offline calls to the sub-account's voicemail.
    Voicemail,
    /// Forward to a PSTN number when the node is unreachable.
    Forward {
        /// The E.164 number to forward to.
        number: String,
    },
    /// No failover — the caller hears an unavailable signal.
    None,
}

/// Voicemail configuration for a sub-account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoicemailConfig {
    /// Whether voicemail is enabled.
    pub enabled: bool,
    /// Where Vitelity emails the voicemail recording, if set.
    pub email: Option<String>,
}

/// A node's Vitelity API credentials — the master login + API key.
///
/// Held by the leader/provisioner only (design lock 7); passed as a
/// *parameter* to [`LiveVitelityClient`](super::live::LiveVitelityClient),
/// never a global and never in process args. Redacted `Debug`, zeroized
/// on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct VitelityCredentials {
    /// The Vitelity account login (username).
    pub login: String,
    /// The Vitelity master API key.
    pub api_key: String,
}

impl VitelityCredentials {
    /// Construct credentials from an owned login + API key.
    #[must_use]
    pub fn new(login: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            login: login.into(),
            api_key: api_key.into(),
        }
    }
}

impl std::fmt::Debug for VitelityCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VitelityCredentials")
            .field("login", &self.login)
            .field("api_key", &"<redacted>")
            .finish()
    }
}
