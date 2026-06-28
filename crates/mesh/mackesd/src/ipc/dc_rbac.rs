//! DATACENTER RBAC — the two-role (viewer/operator) gate every **mutating**
//! `action/dc/*` responder consults before it mutates infrastructure
//! (`docs/design/datacenter-control.md` §9).
//!
//! Principals come from **mesh identity** (the caller's Nebula cert name / peer
//! id, which the Workbench plane stamps into each request body as `principal`)
//! checked against a **role map**. Two effective roles ship — `viewer` (read) and
//! `operator` (do-all) — on a framework with `admin` reserved.
//!
//! **Default-trust (§9 "access to the mesh IS the control plane").** A principal
//! that is NOT explicitly listed in the role map — including an absent/empty
//! principal — resolves to `Operator`. The map only ever *downgrades* an
//! explicitly-listed `viewer`; this keeps the flat-trust envelope (§8) the default
//! while letting an operator pin specific read-only principals. An *unknown* role
//! token (a typo) fails safe to `Viewer` — a misconfigured map can never silently
//! grant write access.
//!
//! Pure: the decision ([`decide`]) is a pure function of `(principal, role_map,
//! mutating)` and is unit-tested; the thin [`authorize`] wrapper extracts the
//! principal from the request body and reads the role map from the environment
//! (`MCNF_DC_ROLE_MAP`) before delegating to it. Read-only verbs (`mutating ==
//! false`) are always allowed — a viewer can read everything.

/// The two effective datacenter roles (design §9). `admin` is reserved by the
/// framework and, until it carries distinct privileges, resolves to `Operator`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Read-only: may invoke read verbs, never a mutating one.
    Viewer,
    /// Do-all: may invoke every verb (the default for an unlisted principal).
    Operator,
}

/// The environment variable holding the role map (see [`role_for`]).
pub const ROLE_MAP_ENV: &str = "MCNF_DC_ROLE_MAP";

/// Resolve the [`Role`] for a mesh `principal` against a `role_map`. PURE.
///
/// `role_map` is a comma-separated list of `<principal>=<role>` (or
/// `<principal>:<role>`) entries — e.g. `"alice=operator,bob=viewer"`. Resolution:
/// * an empty `principal` ⇒ `Operator` (default-trust; the panel always stamps a
///   principal, so an empty one means "unauthenticated local call");
/// * a `principal` NOT present in the map ⇒ `Operator` (default-trust);
/// * an entry mapping it to `viewer` ⇒ `Viewer`;
/// * an entry mapping it to `operator`/`admin` ⇒ `Operator`;
/// * an entry with an UNKNOWN role token ⇒ `Viewer` (fail safe — a typo must not
///   grant write).
///
/// Matching is case-insensitive on the role token and exact on the principal.
/// The first matching entry wins.
#[must_use]
pub fn role_for(principal: &str, role_map: &str) -> Role {
    let principal = principal.trim();
    if principal.is_empty() {
        return Role::Operator;
    }
    for entry in role_map.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // Split on the first '=' or ':' (whichever comes first) so a principal
        // can carry either separator; the value is the role token.
        let sep = match (entry.find('='), entry.find(':')) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let Some(idx) = sep else {
            continue;
        };
        let (p, role) = entry.split_at(idx);
        let p = p.trim();
        let role = role[1..].trim();
        if p != principal {
            continue;
        }
        return match role.to_ascii_lowercase().as_str() {
            "operator" | "admin" => Role::Operator,
            "viewer" => Role::Viewer,
            // Unknown role token: fail safe to the least privilege.
            _ => Role::Viewer,
        };
    }
    // Not listed: default-trust.
    Role::Operator
}

/// Decide whether `principal` may invoke a verb, given the `role_map` and whether
/// the verb is `mutating`. PURE.
///
/// Read-only verbs (`mutating == false`) are always allowed. A mutating verb is
/// allowed for an `Operator` and rejected for a `Viewer`.
///
/// # Errors
/// Returns `Err(reason)` when a `Viewer` attempts a mutating verb.
pub fn decide(principal: &str, role_map: &str, mutating: bool) -> Result<(), String> {
    if !mutating {
        return Ok(());
    }
    match role_for(principal, role_map) {
        Role::Operator => Ok(()),
        Role::Viewer => Err(format!(
            "rbac: principal '{}' has role viewer (read-only); operator role required",
            principal.trim()
        )),
    }
}

/// The configured role map from the environment (`MCNF_DC_ROLE_MAP`), empty when
/// unset (⇒ every principal is an `Operator`, the flat-trust default).
#[must_use]
fn role_map_env() -> String {
    std::env::var(ROLE_MAP_ENV).unwrap_or_default()
}

/// Extract the caller's `principal` from a request body. PURE-ish (just a parse).
/// A missing body, unparseable JSON, or an absent `principal` field yields the
/// empty string (⇒ default-trust `Operator`).
#[must_use]
pub fn principal_of(req_body: Option<&str>) -> String {
    req_body
        .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
        .and_then(|v| {
            v.get("principal")
                .and_then(|p| p.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Authorize a (possibly `mutating`) action for the principal carried in
/// `req_body`, against the environment role map.
///
/// The mutating responders call this FIRST (before any dom0 allow-list / op-lock /
/// SSH) so a viewer's write attempt is rejected without touching the substrate.
///
/// # Errors
/// Returns `Err(reason)` when a `Viewer` attempts a mutating verb.
pub fn authorize(req_body: Option<&str>, mutating: bool) -> Result<(), String> {
    decide(&principal_of(req_body), &role_map_env(), mutating)
}

/// Serializes the env-mutating RBAC integration tests across the crate's test
/// modules (they all compile into one binary, so this static is shared). The
/// role-map var only affects *principal-bearing* calls — a no-principal call is
/// always `Operator` — so only these few tests can collide; this lock holds them
/// apart from each other.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlisted_and_empty_principal_default_to_operator() {
        // Default-trust: an unlisted principal is operator…
        assert_eq!(role_for("alice", "bob=viewer"), Role::Operator);
        // …an empty principal is operator (unauthenticated local call)…
        assert_eq!(role_for("", "bob=viewer"), Role::Operator);
        // …and an empty map makes everyone operator.
        assert_eq!(role_for("anyone", ""), Role::Operator);
    }

    #[test]
    fn explicit_viewer_is_downgraded_operator_and_admin_are_full() {
        assert_eq!(role_for("bob", "alice=operator,bob=viewer"), Role::Viewer);
        assert_eq!(
            role_for("alice", "alice=operator,bob=viewer"),
            Role::Operator
        );
        // admin resolves to Operator (reserved, no distinct privileges yet).
        assert_eq!(role_for("carol", "carol=admin"), Role::Operator);
        // ':' separator works too.
        assert_eq!(role_for("dave", "dave:viewer"), Role::Viewer);
        // case-insensitive role token.
        assert_eq!(role_for("eve", "eve=Viewer"), Role::Viewer);
    }

    #[test]
    fn unknown_role_token_fails_safe_to_viewer() {
        // A typo'd role must NOT grant write — least privilege.
        assert_eq!(role_for("mallory", "mallory=operatr"), Role::Viewer);
        assert_eq!(role_for("mallory", "mallory=root"), Role::Viewer);
    }

    #[test]
    fn first_matching_entry_wins_and_whitespace_tolerated() {
        assert_eq!(
            role_for("bob", " bob = viewer , bob = operator "),
            Role::Viewer
        );
    }

    #[test]
    fn decide_allows_reads_always_and_gates_writes() {
        // Reads always pass, even for a viewer.
        assert!(decide("bob", "bob=viewer", false).is_ok());
        // A viewer's write is rejected with a clear reason…
        let e = decide("bob", "bob=viewer", true).unwrap_err();
        assert!(e.contains("rbac"), "{e}");
        assert!(e.contains("viewer"), "{e}");
        assert!(e.contains("bob"), "{e}");
        // …an operator's write passes…
        assert!(decide("alice", "alice=operator", true).is_ok());
        // …and an unlisted principal (default-trust) writes fine.
        assert!(decide("alice", "bob=viewer", true).is_ok());
    }

    #[test]
    fn principal_of_reads_the_body_field() {
        assert_eq!(
            principal_of(Some(r#"{"principal":"alice","uuid":"x"}"#)),
            "alice"
        );
        // Missing principal / body / bad json → empty (default-trust).
        assert_eq!(principal_of(Some(r#"{"uuid":"x"}"#)), "");
        assert_eq!(principal_of(None), "");
        assert_eq!(principal_of(Some("not json")), "");
    }
}
