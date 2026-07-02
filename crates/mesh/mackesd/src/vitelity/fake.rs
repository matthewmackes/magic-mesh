//! An in-memory [`VitelityClient`] double for tests + headless
//! reconcile exercising (VOIP-GW-2).
//!
//! [`FakeVitelityClient`] records the operations the VOIP-GW-3 worker
//! drives and answers from in-memory state — no network. It is honest
//! about failures too: preload it with [`FakeVitelityClient::with_did`]
//! and route/failover/voicemail calls mutate that state so a test can
//! assert the worker's reconcile reached the desired end state.

use std::cell::RefCell;
use std::collections::HashMap;

use super::model::{
    CreateSubAccount, Did, DidRouting, FailoverPolicy, SubAccount, SubAccountCredentials,
    SubAccountSummary, VoicemailConfig,
};
use super::{VitelityClient, VitelityError};

/// In-memory Vitelity double. Uses interior mutability so it satisfies
/// the `&self` trait without forcing callers to hold a `&mut`.
#[derive(Debug, Default)]
pub struct FakeVitelityClient {
    state: RefCell<FakeState>,
}

#[derive(Debug, Default)]
struct FakeState {
    sub_accounts: Vec<SubAccount>,
    passwords: HashMap<String, String>,
    dids: Vec<Did>,
    failover: HashMap<String, FailoverPolicy>,
    voicemail: HashMap<String, VoicemailConfig>,
    realm: String,
}

impl FakeVitelityClient {
    /// A fresh double with the given SIP realm (forms `<user>@<realm>`).
    #[must_use]
    pub fn new(realm: impl Into<String>) -> Self {
        Self {
            state: RefCell::new(FakeState {
                realm: realm.into(),
                ..FakeState::default()
            }),
        }
    }

    /// Preload an existing master-account DID (lock 11 — the fake never
    /// invents DIDs, matching the live client's list-only contract).
    #[must_use]
    pub fn with_did(self, number: impl Into<String>, routed_to: Option<String>) -> Self {
        self.state.borrow_mut().dids.push(Did {
            number: number.into(),
            routed_to,
        });
        self
    }

    /// The current failover policy recorded for `username`, if any
    /// (test assertion helper).
    #[must_use]
    pub fn failover_of(&self, username: &str) -> Option<FailoverPolicy> {
        self.state.borrow().failover.get(username).cloned()
    }

    /// The current voicemail config recorded for `username`, if any.
    #[must_use]
    pub fn voicemail_of(&self, username: &str) -> Option<VoicemailConfig> {
        self.state.borrow().voicemail.get(username).cloned()
    }
}

impl VitelityClient for FakeVitelityClient {
    fn create_sub_account(
        &self,
        req: &CreateSubAccount,
    ) -> Result<(SubAccount, SubAccountCredentials), VitelityError> {
        let mut st = self.state.borrow_mut();
        if st.sub_accounts.iter().any(|s| s.username == req.username) {
            return Err(VitelityError::Api {
                op: "create-subaccount",
                message: format!("sub-account {} already exists", req.username),
            });
        }
        let account = SubAccount {
            username: req.username.clone(),
            description: req.description.clone(),
            realm: st.realm.clone(),
        };
        let password = format!("fake-pw-{}", req.username);
        st.passwords.insert(req.username.clone(), password.clone());
        st.sub_accounts.push(account.clone());
        Ok((
            account,
            SubAccountCredentials {
                username: req.username.clone(),
                sip_password: password,
            },
        ))
    }

    fn list_sub_accounts(&self) -> Result<Vec<SubAccountSummary>, VitelityError> {
        Ok(self
            .state
            .borrow()
            .sub_accounts
            .iter()
            .map(|s| SubAccountSummary {
                username: s.username.clone(),
                description: s.description.clone(),
            })
            .collect())
    }

    fn get_sub_account(&self, username: &str) -> Result<SubAccount, VitelityError> {
        self.state
            .borrow()
            .sub_accounts
            .iter()
            .find(|s| s.username == username)
            .cloned()
            .ok_or_else(|| VitelityError::Api {
                op: "get-subaccount",
                message: format!("no such sub-account {username}"),
            })
    }

    fn list_dids(&self) -> Result<Vec<Did>, VitelityError> {
        Ok(self.state.borrow().dids.clone())
    }

    fn route_did(&self, did: &str, routing: &DidRouting) -> Result<(), VitelityError> {
        let mut st = self.state.borrow_mut();
        let target = match routing {
            DidRouting::SubAccount(u) => Some(u.clone()),
            DidRouting::MainAccount => None,
        };
        let row = st.dids.iter_mut().find(|d| d.number == did);
        match row {
            Some(d) => {
                d.routed_to = target;
                Ok(())
            }
            None => Err(VitelityError::Api {
                op: "route-did",
                message: format!("no such DID {did} on the master account"),
            }),
        }
    }

    fn configure_failover(
        &self,
        username: &str,
        policy: &FailoverPolicy,
    ) -> Result<(), VitelityError> {
        self.state
            .borrow_mut()
            .failover
            .insert(username.to_string(), policy.clone());
        Ok(())
    }

    fn configure_voicemail(
        &self,
        username: &str,
        vm: &VoicemailConfig,
    ) -> Result<(), VitelityError> {
        self.state
            .borrow_mut()
            .voicemail
            .insert(username.to_string(), vm.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_list_and_get_round_trip() {
        let c = FakeVitelityClient::new("sip.vitelity.net");
        let req = CreateSubAccount {
            username: "node-eagle".to_string(),
            description: "Eagle".to_string(),
        };
        let (acct, creds) = c.create_sub_account(&req).unwrap();
        assert_eq!(acct.realm, "sip.vitelity.net");
        assert_eq!(creds.sip_password, "fake-pw-node-eagle");

        let list = c.list_sub_accounts().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].username, "node-eagle");

        assert_eq!(c.get_sub_account("node-eagle").unwrap(), acct);
        assert!(matches!(
            c.get_sub_account("nope").unwrap_err(),
            VitelityError::Api { .. }
        ));
    }

    #[test]
    fn duplicate_create_is_a_typed_error() {
        let c = FakeVitelityClient::new("sip.vitelity.net");
        let req = CreateSubAccount {
            username: "node-eagle".to_string(),
            description: "Eagle".to_string(),
        };
        c.create_sub_account(&req).unwrap();
        assert!(matches!(
            c.create_sub_account(&req).unwrap_err(),
            VitelityError::Api {
                op: "create-subaccount",
                ..
            }
        ));
    }

    #[test]
    fn route_existing_did_and_reject_unknown() {
        let c = FakeVitelityClient::new("sip.vitelity.net").with_did("15551234567", None);
        assert_eq!(c.list_dids().unwrap()[0].routed_to, None);

        c.route_did("15551234567", &DidRouting::SubAccount("node-eagle".into()))
            .unwrap();
        assert_eq!(
            c.list_dids().unwrap()[0].routed_to,
            Some("node-eagle".to_string())
        );

        assert!(matches!(
            c.route_did("19999999999", &DidRouting::MainAccount)
                .unwrap_err(),
            VitelityError::Api {
                op: "route-did",
                ..
            }
        ));
    }

    #[test]
    fn failover_and_voicemail_are_recorded() {
        let c = FakeVitelityClient::new("sip.vitelity.net");
        c.configure_failover("node-eagle", &FailoverPolicy::Voicemail)
            .unwrap();
        assert_eq!(c.failover_of("node-eagle"), Some(FailoverPolicy::Voicemail));

        let vm = VoicemailConfig {
            enabled: true,
            email: Some("vm@example.com".to_string()),
        };
        c.configure_voicemail("node-eagle", &vm).unwrap();
        assert_eq!(c.voicemail_of("node-eagle"), Some(vm));
    }
}
