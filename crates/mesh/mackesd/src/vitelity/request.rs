//! Pure request-building folds for the Vitelity client (VOIP-GW-2).
//!
//! Each `build_*` returns a [`VitelityRequest`] — the Vitelity `cmd`
//! plus the **non-secret** business query params. Credentials are NOT
//! carried here: the live transport injects `login` + the API key at
//! send time (see [`VitelityRequest::to_query`]), so a request is safe
//! to log. All functions are pure and unit-tested against the folds'
//! own output — no network.

use super::model::{
    CreateSubAccount, DidRouting, FailoverPolicy, VitelityCredentials, VoicemailConfig,
};

/// The Vitelity provisioning API endpoint (design: Vitelity HTTP API).
pub const VITELITY_ENDPOINT: &str = "https://api.vitelity.net/api.php";

/// A built, credential-free Vitelity request: the command + business
/// params. `Debug`/`Display` never carry secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VitelityRequest {
    /// The Vitelity `cmd` token (e.g. `createSubAccount`).
    pub cmd: &'static str,
    /// Ordered non-secret business params (key, value).
    pub params: Vec<(String, String)>,
}

impl VitelityRequest {
    const fn new(cmd: &'static str, params: Vec<(String, String)>) -> Self {
        Self { cmd, params }
    }

    /// Assemble the full `application/x-www-form-urlencoded` query
    /// string, injecting the credentials (`login`, `pass`) + `cmd`
    /// ahead of the business params.
    ///
    /// This is the ONE place credentials enter the wire form; the
    /// result is passed straight to the transport and never logged.
    #[must_use]
    pub fn to_query(&self, creds: &VitelityCredentials) -> String {
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(self.params.len() + 3);
        pairs.push(("login".to_string(), creds.login.clone()));
        pairs.push(("pass".to_string(), creds.api_key.clone()));
        pairs.push(("cmd".to_string(), self.cmd.to_string()));
        pairs.extend(self.params.iter().cloned());
        encode_form(&pairs)
    }

    /// The non-secret param assembly used for logging / tests — the
    /// `cmd` and business params, with **no** credentials.
    #[must_use]
    pub fn to_logged_query(&self) -> String {
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(self.params.len() + 1);
        pairs.push(("cmd".to_string(), self.cmd.to_string()));
        pairs.extend(self.params.iter().cloned());
        encode_form(&pairs)
    }
}

/// `application/x-www-form-urlencoded` encode an ordered pair list.
///
/// Pure. Encodes every byte outside the unreserved set
/// (`A-Z a-z 0-9 - _ . ~`) as `%XX`; space stays `%20` (not `+`) so the
/// encoding round-trips through a plain percent-decoder.
#[must_use]
pub fn encode_form(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(k));
        out.push('=');
        out.push_str(&percent_encode(v));
    }
    out
}

/// Percent-encode one component (RFC 3986 unreserved set kept literal).
#[must_use]
pub fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// Build the create-sub-account request.
#[must_use]
pub fn build_create_sub_account(req: &CreateSubAccount) -> VitelityRequest {
    VitelityRequest::new(
        "createSubAccount",
        vec![
            ("sub_account".to_string(), req.username.clone()),
            ("description".to_string(), req.description.clone()),
            // Receive-only (lock 4): outbound is bridged onto the shared
            // trunk Vitelity-side, so the sub-account itself is inbound.
            ("type".to_string(), "inbound".to_string()),
        ],
    )
}

/// Build the list-sub-accounts request.
#[must_use]
pub const fn build_list_sub_accounts() -> VitelityRequest {
    VitelityRequest::new("listSubAccounts", Vec::new())
}

/// Build the get-one-sub-account request.
#[must_use]
pub fn build_get_sub_account(username: &str) -> VitelityRequest {
    VitelityRequest::new(
        "getSubAccount",
        vec![("sub_account".to_string(), username.to_string())],
    )
}

/// Build the list-existing-DIDs request (lock 11).
#[must_use]
pub const fn build_list_dids() -> VitelityRequest {
    VitelityRequest::new("listDIDs", Vec::new())
}

/// Build the route-existing-DID request (lock 11 — routes, never
/// provisions).
#[must_use]
pub fn build_route_did(did: &str, routing: &DidRouting) -> VitelityRequest {
    let target = match routing {
        DidRouting::SubAccount(u) => u.clone(),
        DidRouting::MainAccount => "main".to_string(),
    };
    VitelityRequest::new(
        "routeDID",
        vec![
            ("did".to_string(), did.to_string()),
            ("routesip".to_string(), target),
        ],
    )
}

/// Build the failover-config request (lock 10).
#[must_use]
pub fn build_configure_failover(username: &str, policy: &FailoverPolicy) -> VitelityRequest {
    let mut params = vec![("sub_account".to_string(), username.to_string())];
    match policy {
        FailoverPolicy::Voicemail => {
            params.push(("failover".to_string(), "voicemail".to_string()));
        }
        FailoverPolicy::Forward { number } => {
            params.push(("failover".to_string(), "forward".to_string()));
            params.push(("forward".to_string(), number.clone()));
        }
        FailoverPolicy::None => {
            params.push(("failover".to_string(), "none".to_string()));
        }
    }
    VitelityRequest::new("subAccountFailover", params)
}

/// Build the voicemail-config request.
#[must_use]
pub fn build_configure_voicemail(username: &str, vm: &VoicemailConfig) -> VitelityRequest {
    let mut params = vec![
        ("sub_account".to_string(), username.to_string()),
        (
            "voicemail".to_string(),
            if vm.enabled { "yes" } else { "no" }.to_string(),
        ),
    ];
    if let Some(email) = &vm.email {
        params.push(("email".to_string(), email.clone()));
    }
    VitelityRequest::new("subAccountVoicemail", params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_keeps_unreserved_and_escapes_rest() {
        assert_eq!(percent_encode("node-eagle_1.0~x"), "node-eagle_1.0~x");
        assert_eq!(percent_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(percent_encode("k@ey/+"), "k%40ey%2F%2B");
    }

    #[test]
    fn create_sub_account_builds_inbound_only() {
        let r = build_create_sub_account(&CreateSubAccount {
            username: "node-eagle".to_string(),
            description: "Eagle".to_string(),
        });
        assert_eq!(r.cmd, "createSubAccount");
        assert_eq!(
            r.params,
            vec![
                ("sub_account".to_string(), "node-eagle".to_string()),
                ("description".to_string(), "Eagle".to_string()),
                ("type".to_string(), "inbound".to_string()),
            ]
        );
    }

    #[test]
    fn logged_query_has_no_credentials() {
        let r = build_get_sub_account("node-eagle");
        let logged = r.to_logged_query();
        assert_eq!(logged, "cmd=getSubAccount&sub_account=node-eagle");
        assert!(!logged.contains("login"));
        assert!(!logged.contains("pass"));
    }

    #[test]
    fn to_query_injects_credentials_first() {
        let r = build_list_sub_accounts();
        let creds = VitelityCredentials::new("acct42", "SECRET-KEY&raw");
        let q = r.to_query(&creds);
        // login + pass + cmd lead; the raw key is percent-encoded.
        assert_eq!(q, "login=acct42&pass=SECRET-KEY%26raw&cmd=listSubAccounts");
    }

    #[test]
    fn route_did_targets_subaccount_or_main() {
        let sub = build_route_did("15551234567", &DidRouting::SubAccount("node-eagle".into()));
        assert_eq!(sub.cmd, "routeDID");
        assert_eq!(
            sub.params,
            vec![
                ("did".to_string(), "15551234567".to_string()),
                ("routesip".to_string(), "node-eagle".to_string()),
            ]
        );
        let main = build_route_did("15551234567", &DidRouting::MainAccount);
        assert_eq!(main.params[1], ("routesip".to_string(), "main".to_string()));
    }

    #[test]
    fn failover_forward_carries_number() {
        let fwd = build_configure_failover(
            "node-eagle",
            &FailoverPolicy::Forward {
                number: "15559876543".to_string(),
            },
        );
        assert_eq!(fwd.cmd, "subAccountFailover");
        assert!(fwd
            .params
            .contains(&("failover".to_string(), "forward".to_string())));
        assert!(fwd
            .params
            .contains(&("forward".to_string(), "15559876543".to_string())));

        let vm = build_configure_failover("node-eagle", &FailoverPolicy::Voicemail);
        assert!(vm
            .params
            .contains(&("failover".to_string(), "voicemail".to_string())));
    }

    #[test]
    fn voicemail_config_encodes_enabled_and_email() {
        let on = build_configure_voicemail(
            "node-eagle",
            &VoicemailConfig {
                enabled: true,
                email: Some("vm@example.com".to_string()),
            },
        );
        assert!(on
            .params
            .contains(&("voicemail".to_string(), "yes".to_string())));
        assert!(on
            .params
            .contains(&("email".to_string(), "vm@example.com".to_string())));

        let off = build_configure_voicemail(
            "node-eagle",
            &VoicemailConfig {
                enabled: false,
                email: None,
            },
        );
        assert!(off
            .params
            .contains(&("voicemail".to_string(), "no".to_string())));
        assert!(!off.params.iter().any(|(k, _)| k == "email"));
    }
}
