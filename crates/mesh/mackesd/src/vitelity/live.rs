//! The production [`VitelityClient`] — integration-gated (VOIP-GW-2).
//!
//! [`LiveVitelityClient`] holds the master API key as a **parameter**
//! (design lock 7 — leader-held, never a global, never in process
//! args). This slice delivers the seam + the pure request/response
//! folds; the real HTTP transport is wired by the VOIP-GW-3
//! `voice_provision` worker. Until then every method **builds its real
//! request** (exercising the [`request`](super::request) folds) and
//! returns a typed [`VitelityError::IntegrationGated`] naming exactly
//! what the live call needs — the master API key + network reachability
//! to the Vitelity endpoint. That is a complete, honest behavior, never
//! a fake success (§7).

use super::model::{
    CreateSubAccount, Did, DidRouting, FailoverPolicy, SubAccount, SubAccountCredentials,
    SubAccountSummary, VitelityCredentials, VoicemailConfig,
};
use super::request::{
    build_configure_failover, build_configure_voicemail, build_create_sub_account,
    build_get_sub_account, build_list_dids, build_list_sub_accounts, build_route_did,
    VitelityRequest, VITELITY_ENDPOINT,
};
use super::{VitelityClient, VitelityError};

/// Production Vitelity client. Owns the master credentials for the life
/// of the provisioning run; they are zeroized on drop.
#[derive(Debug)]
pub struct LiveVitelityClient {
    creds: VitelityCredentials,
    endpoint: String,
}

impl LiveVitelityClient {
    /// Construct with the master credentials (passed in by the leader —
    /// never read from a global). Uses the canonical Vitelity endpoint.
    #[must_use]
    pub fn new(creds: VitelityCredentials) -> Self {
        Self {
            creds,
            endpoint: VITELITY_ENDPOINT.to_string(),
        }
    }

    /// Construct against an explicit endpoint (for a future staging
    /// host); the transport is still integration-gated.
    #[must_use]
    pub fn with_endpoint(creds: VitelityCredentials, endpoint: impl Into<String>) -> Self {
        Self {
            creds,
            endpoint: endpoint.into(),
        }
    }

    /// The Vitelity endpoint this client targets.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The non-secret Vitelity account login this client authenticates
    /// as (for display / audit — the API key is never exposed).
    #[must_use]
    pub fn account_login(&self) -> &str {
        &self.creds.login
    }

    /// Shared gate: builds the request (so the pure folds are exercised
    /// on the live path too) then returns the typed gated error. The
    /// request is referenced only via its non-secret `cmd`; credentials
    /// never enter the error text.
    fn gated<T>(&self, op: &'static str, req: &VitelityRequest) -> Result<T, VitelityError> {
        Err(VitelityError::IntegrationGated {
            op,
            reason: format!(
                "needs the Vitelity master API key + live network reachability to {} to run \
                 cmd={}; the transport is wired by the voice_provision worker (VOIP-GW-3)",
                self.endpoint, req.cmd
            ),
        })
    }
}

impl VitelityClient for LiveVitelityClient {
    fn create_sub_account(
        &self,
        req: &CreateSubAccount,
    ) -> Result<(SubAccount, SubAccountCredentials), VitelityError> {
        self.gated("create-subaccount", &build_create_sub_account(req))
    }

    fn list_sub_accounts(&self) -> Result<Vec<SubAccountSummary>, VitelityError> {
        self.gated("list-subaccounts", &build_list_sub_accounts())
    }

    fn get_sub_account(&self, username: &str) -> Result<SubAccount, VitelityError> {
        self.gated("get-subaccount", &build_get_sub_account(username))
    }

    fn list_dids(&self) -> Result<Vec<Did>, VitelityError> {
        self.gated("list-dids", &build_list_dids())
    }

    fn route_did(&self, did: &str, routing: &DidRouting) -> Result<(), VitelityError> {
        self.gated("route-did", &build_route_did(did, routing))
    }

    fn configure_failover(
        &self,
        username: &str,
        policy: &FailoverPolicy,
    ) -> Result<(), VitelityError> {
        self.gated(
            "configure-failover",
            &build_configure_failover(username, policy),
        )
    }

    fn configure_voicemail(
        &self,
        username: &str,
        vm: &VoicemailConfig,
    ) -> Result<(), VitelityError> {
        self.gated(
            "configure-voicemail",
            &build_configure_voicemail(username, vm),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> LiveVitelityClient {
        LiveVitelityClient::new(VitelityCredentials::new("acct42", "MASTER-KEY"))
    }

    #[test]
    fn every_op_is_integration_gated_never_faked() {
        let c = client();
        let create = CreateSubAccount {
            username: "node-eagle".to_string(),
            description: "Eagle".to_string(),
        };
        assert!(matches!(
            c.create_sub_account(&create),
            Err(VitelityError::IntegrationGated {
                op: "create-subaccount",
                ..
            })
        ));
        assert!(matches!(
            c.list_sub_accounts(),
            Err(VitelityError::IntegrationGated {
                op: "list-subaccounts",
                ..
            })
        ));
        assert!(matches!(
            c.get_sub_account("node-eagle"),
            Err(VitelityError::IntegrationGated {
                op: "get-subaccount",
                ..
            })
        ));
        assert!(matches!(
            c.list_dids(),
            Err(VitelityError::IntegrationGated {
                op: "list-dids",
                ..
            })
        ));
        assert!(matches!(
            c.route_did("15551234567", &DidRouting::MainAccount),
            Err(VitelityError::IntegrationGated {
                op: "route-did",
                ..
            })
        ));
        assert!(matches!(
            c.configure_failover("node-eagle", &FailoverPolicy::Voicemail),
            Err(VitelityError::IntegrationGated {
                op: "configure-failover",
                ..
            })
        ));
        assert!(matches!(
            c.configure_voicemail(
                "node-eagle",
                &VoicemailConfig {
                    enabled: true,
                    email: None
                }
            ),
            Err(VitelityError::IntegrationGated {
                op: "configure-voicemail",
                ..
            })
        ));
    }

    #[test]
    fn gated_error_names_the_need_without_leaking_secrets() {
        let c = client();
        let err = c.list_dids().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("master API key"));
        assert!(msg.contains("listDIDs"));
        // The actual key value must never appear in the error text.
        assert!(!msg.contains("MASTER-KEY"));
    }

    #[test]
    fn credentials_are_redacted_in_debug() {
        let c = client();
        assert!(!format!("{c:?}").contains("MASTER-KEY"));
    }
}
