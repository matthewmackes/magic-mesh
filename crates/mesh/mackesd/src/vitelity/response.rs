//! Pure response-parsing folds for the Vitelity client (VOIP-GW-2).
//!
//! Vitelity replies with XML wrapped in a `<VitelityResponse>` element
//! carrying a `<status>` of `ok` or `error`. These functions turn a
//! response *string* into the typed model structs (or a typed
//! [`VitelityError`]) with no I/O — unit-tested against fixture strings.

use super::model::{Did, SubAccount, SubAccountCredentials, SubAccountSummary};
use super::VitelityError;

/// Parse + status-check the root `<VitelityResponse>`.
///
/// Returns the root element node on `status == ok`; on `status ==
/// error` returns [`VitelityError::Api`] with Vitelity's `<error>`
/// text; on malformed XML / missing status returns
/// [`VitelityError::Parse`].
fn root<'a>(
    doc: &'a roxmltree::Document<'a>,
    op: &'static str,
) -> Result<roxmltree::Node<'a, 'a>, VitelityError> {
    let root = doc.root_element();
    let status = child_text(root, "status").ok_or_else(|| VitelityError::Parse {
        op,
        detail: "response has no <status> element".to_string(),
    })?;
    if status.eq_ignore_ascii_case("ok") {
        Ok(root)
    } else {
        let message = child_text(root, "error").unwrap_or_else(|| "unspecified error".to_string());
        Err(VitelityError::Api { op, message })
    }
}

fn parse_doc<'a>(xml: &'a str, op: &'static str) -> Result<roxmltree::Document<'a>, VitelityError> {
    roxmltree::Document::parse(xml).map_err(|e| VitelityError::Parse {
        op,
        detail: format!("malformed XML: {e}"),
    })
}

/// The trimmed text of the first direct-or-nested child named `tag`.
fn child_text(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    node.descendants()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
}

/// Parse a status-only response (route-DID, failover, voicemail):
/// success is `Ok(())`, otherwise the typed error.
///
/// # Errors
/// [`VitelityError::Api`] / [`VitelityError::Parse`] per [`root`].
pub fn parse_status(xml: &str, op: &'static str) -> Result<(), VitelityError> {
    let doc = parse_doc(xml, op)?;
    root(&doc, op)?;
    Ok(())
}

/// Parse a create-sub-account response into the record + its one-time
/// SIP credentials.
///
/// # Errors
/// [`VitelityError`] on non-`ok` status or a missing field.
pub fn parse_create_sub_account(
    xml: &str,
    op: &'static str,
) -> Result<(SubAccount, SubAccountCredentials), VitelityError> {
    let doc = parse_doc(xml, op)?;
    let root = root(&doc, op)?;
    let sub = root
        .descendants()
        .find(|n| n.has_tag_name("subaccount"))
        .ok_or_else(|| VitelityError::Parse {
            op,
            detail: "response has no <subaccount>".to_string(),
        })?;
    let username = child_text(sub, "username").ok_or_else(|| VitelityError::Parse {
        op,
        detail: "<subaccount> has no <username>".to_string(),
    })?;
    let sip_password = child_text(sub, "sippassword").ok_or_else(|| VitelityError::Parse {
        op,
        detail: "<subaccount> has no <sippassword>".to_string(),
    })?;
    let account = SubAccount {
        username: username.clone(),
        description: child_text(sub, "description").unwrap_or_default(),
        realm: child_text(sub, "realm").unwrap_or_default(),
    };
    let creds = SubAccountCredentials {
        username,
        sip_password,
    };
    Ok((account, creds))
}

/// Parse a single get-sub-account response.
///
/// # Errors
/// [`VitelityError`] on non-`ok` status or a missing field.
pub fn parse_sub_account(xml: &str, op: &'static str) -> Result<SubAccount, VitelityError> {
    let doc = parse_doc(xml, op)?;
    let root = root(&doc, op)?;
    let sub = root
        .descendants()
        .find(|n| n.has_tag_name("subaccount"))
        .ok_or_else(|| VitelityError::Parse {
            op,
            detail: "response has no <subaccount>".to_string(),
        })?;
    let username = child_text(sub, "username").ok_or_else(|| VitelityError::Parse {
        op,
        detail: "<subaccount> has no <username>".to_string(),
    })?;
    Ok(SubAccount {
        username,
        description: child_text(sub, "description").unwrap_or_default(),
        realm: child_text(sub, "realm").unwrap_or_default(),
    })
}

/// Parse a list-sub-accounts response into compact rows.
///
/// # Errors
/// [`VitelityError`] on non-`ok` status or a row missing `<username>`.
pub fn parse_sub_account_list(
    xml: &str,
    op: &'static str,
) -> Result<Vec<SubAccountSummary>, VitelityError> {
    let doc = parse_doc(xml, op)?;
    let root = root(&doc, op)?;
    let mut out = Vec::new();
    for sub in root.descendants().filter(|n| n.has_tag_name("subaccount")) {
        let username = child_text(sub, "username").ok_or_else(|| VitelityError::Parse {
            op,
            detail: "a <subaccount> row has no <username>".to_string(),
        })?;
        out.push(SubAccountSummary {
            username,
            description: child_text(sub, "description").unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Parse a list-DIDs response (existing DIDs + current routing, lock 11).
///
/// A DID with an empty/absent `<routing>` is routed to the main account
/// ([`Did::routed_to`] is `None`).
///
/// # Errors
/// [`VitelityError`] on non-`ok` status or a row missing `<number>`.
pub fn parse_did_list(xml: &str, op: &'static str) -> Result<Vec<Did>, VitelityError> {
    let doc = parse_doc(xml, op)?;
    let root = root(&doc, op)?;
    let mut out = Vec::new();
    for did in root.descendants().filter(|n| n.has_tag_name("did")) {
        let number = child_text(did, "number").ok_or_else(|| VitelityError::Parse {
            op,
            detail: "a <did> row has no <number>".to_string(),
        })?;
        let routed_to = child_text(did, "routing").filter(|s| !s.is_empty());
        out.push(Did { number, routed_to });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OP: &str = "test-op";

    #[test]
    fn status_ok_and_error_and_malformed() {
        assert!(parse_status(
            "<VitelityResponse><status>ok</status></VitelityResponse>",
            OP
        )
        .is_ok());

        let err = parse_status(
            "<VitelityResponse><status>error</status><error>Invalid API key</error></VitelityResponse>",
            OP,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VitelityError::Api {
                op: OP,
                message: "Invalid API key".to_string()
            }
        );

        assert!(matches!(
            parse_status("<not-xml", OP).unwrap_err(),
            VitelityError::Parse { .. }
        ));
        assert!(matches!(
            parse_status("<VitelityResponse></VitelityResponse>", OP).unwrap_err(),
            VitelityError::Parse { .. }
        ));
    }

    #[test]
    fn create_sub_account_yields_record_and_creds() {
        let xml = "<VitelityResponse><status>ok</status>\
            <subaccount>\
            <username>node-eagle</username>\
            <description>Eagle</description>\
            <realm>sip.vitelity.net</realm>\
            <sippassword>s3cr3t-pw</sippassword>\
            </subaccount></VitelityResponse>";
        let (acct, creds) = parse_create_sub_account(xml, OP).unwrap();
        assert_eq!(
            acct,
            SubAccount {
                username: "node-eagle".to_string(),
                description: "Eagle".to_string(),
                realm: "sip.vitelity.net".to_string(),
            }
        );
        assert_eq!(creds.username, "node-eagle");
        assert_eq!(creds.sip_password, "s3cr3t-pw");
        // Redaction: the secret never appears in Debug.
        assert!(!format!("{creds:?}").contains("s3cr3t-pw"));
    }

    #[test]
    fn create_missing_password_is_parse_error() {
        let xml = "<VitelityResponse><status>ok</status>\
            <subaccount><username>node-eagle</username></subaccount></VitelityResponse>";
        assert!(matches!(
            parse_create_sub_account(xml, OP).unwrap_err(),
            VitelityError::Parse { .. }
        ));
    }

    #[test]
    fn get_sub_account_parses() {
        let xml = "<VitelityResponse><status>ok</status>\
            <subaccount><username>node-fra</username><description>Fra1</description>\
            <realm>sip.vitelity.net</realm></subaccount></VitelityResponse>";
        let acct = parse_sub_account(xml, OP).unwrap();
        assert_eq!(acct.username, "node-fra");
        assert_eq!(acct.realm, "sip.vitelity.net");
    }

    #[test]
    fn list_sub_accounts_parses_rows() {
        let xml = "<VitelityResponse><status>ok</status><subaccounts>\
            <subaccount><username>node-eagle</username><description>Eagle</description></subaccount>\
            <subaccount><username>node-fra</username><description>Fra1</description></subaccount>\
            </subaccounts></VitelityResponse>";
        let rows = parse_sub_account_list(xml, OP).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].username, "node-eagle");
        assert_eq!(rows[1].description, "Fra1");
    }

    #[test]
    fn list_dids_parses_routing_and_main() {
        let xml = "<VitelityResponse><status>ok</status><dids>\
            <did><number>15551234567</number><routing>node-eagle</routing></did>\
            <did><number>15550000000</number><routing></routing></did>\
            </dids></VitelityResponse>";
        let dids = parse_did_list(xml, OP).unwrap();
        assert_eq!(dids.len(), 2);
        assert_eq!(
            dids[0],
            Did {
                number: "15551234567".to_string(),
                routed_to: Some("node-eagle".to_string()),
            }
        );
        assert_eq!(dids[1].routed_to, None, "empty routing == main account");
    }

    #[test]
    fn error_status_propagates_through_typed_parsers() {
        let xml = "<VitelityResponse><status>error</status><error>rate limited</error></VitelityResponse>";
        assert!(matches!(
            parse_did_list(xml, OP).unwrap_err(),
            VitelityError::Api { message, .. } if message == "rate limited"
        ));
    }
}
