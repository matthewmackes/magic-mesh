//! DATACENTER RBAC — the two-role (viewer/operator) gate every **mutating**
//! `action/dc/*` responder consults before it mutates infrastructure
//! (`docs/design/datacenter-control.md` §9).
//!
//! Principals come from **mesh identity** (the caller's Nebula cert name / peer
//! id, which the Workbench plane stamps into each request body as `principal`).
//! Two effective roles ship — `viewer` (read) and `operator` (do-all) — on a
//! framework with `admin` reserved (today identical to `operator`).
//!
//! ## Two gate styles (both exposed; one module, one [`Role`] type)
//!
//! Two DATACENTER backends grew slightly different convenience wrappers; both
//! survive here so every responder keeps its existing call style with zero churn,
//! and both deny + audit consistently:
//!
//! * [`authorize`]`(req_body, mutating)` — the responder already knows whether the
//!   verb mutates (each carries its own `is_mutating`). It resolves the principal
//!   against the **`MCNF_DC_ROLE_MAP`** map ([`role_for`]/[`decide`]) under
//!   **default-trust** semantics: an unlisted/absent principal is `Operator`; the
//!   map only ever *downgrades* an explicitly-listed `viewer` (the flat-trust
//!   envelope, §8). Used by the VM / host / storage / network responders.
//! * [`enforce`]`(verb, req_body)` (+ [`audit_denial`]) — classifies the verb
//!   itself ([`verb_is_mutating`], deny-by-default) and resolves the principal
//!   against the **`MCNF_DC_ROLES`** + **`MCNF_DC_DEFAULT_ROLE`** policy
//!   ([`RoleMap`]) under **fail-closed-when-configured** semantics: with no policy
//!   set the plane is transparent single-operator mode; once a policy exists an
//!   unlisted/absent principal falls to the (viewer) default. Used by the power /
//!   provision / tofu responders, which also append a tamper-evident
//!   `event/dc/audit/*` deny record on refusal.
//!
//! Both styles share this [`Role`] type and reject a `viewer`'s mutation; they
//! differ only in *where the policy is read from* and the unlisted-principal
//! default. The pure cores ([`decide`], [`RoleMap`]) are unit-tested below.

use std::collections::BTreeMap;

/// A Datacenter-plane role. Two ship (`Viewer`, `Operator`); `Admin` is the
/// reserved third tier on the framework (today it grants the same as `Operator`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Read-only: may run the read verbs, denied every mutation.
    Viewer,
    /// Do-all: may run every verb (the default for an unlisted principal under
    /// the `authorize`/`MCNF_DC_ROLE_MAP` default-trust style).
    Operator,
    /// Reserved tier; today identical to [`Role::Operator`] for authorization.
    Admin,
}

impl Role {
    /// May this role run a mutating verb? `Operator`/`Admin` yes, `Viewer` no.
    #[must_use]
    pub const fn can_mutate(self) -> bool {
        matches!(self, Self::Operator | Self::Admin)
    }

    /// The wire/log token for this role.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    /// Parse a role token (case-insensitive). PURE. `None` for an unknown token.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "viewer" | "read" | "readonly" | "read-only" => Some(Self::Viewer),
            "operator" | "op" | "all" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ===========================================================================
// authorize style — `MCNF_DC_ROLE_MAP`, default-trust (unlisted ⇒ Operator).
// ===========================================================================

/// The environment variable holding the [`authorize`]-style role map (see
/// [`role_for`]).
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
/// allowed for an `Operator`/`Admin` and rejected for a `Viewer`.
///
/// # Errors
/// Returns `Err(reason)` when a `Viewer` attempts a mutating verb.
pub fn decide(principal: &str, role_map: &str, mutating: bool) -> Result<(), String> {
    if !mutating {
        return Ok(());
    }
    if role_for(principal, role_map).can_mutate() {
        Ok(())
    } else {
        Err(format!(
            "rbac: principal '{}' has role viewer (read-only); operator role required",
            principal.trim()
        ))
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
/// `req_body`, against the environment role map (`MCNF_DC_ROLE_MAP`).
///
/// The mutating responders call this FIRST (before any dom0 allow-list / op-lock /
/// SSH) so a viewer's write attempt is rejected without touching the substrate.
/// On a denial the responder may also [`audit_denial`] the refusal.
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

// ===========================================================================
// enforce style — `MCNF_DC_ROLES` + `MCNF_DC_DEFAULT_ROLE`, deny-by-default
// verb classifier, fail-closed-when-configured, + audited denials.
// ===========================================================================

/// The read-only `action/dc/*` verbs — allowed for any resolved role (a viewer
/// may read). Everything NOT in this set is treated as a mutation and requires
/// `operator`/`admin` (deny-by-default for unknown verbs). Used by [`enforce`].
///
/// Kept in sync with the per-responder verb tables:
/// * [`crate::ipc::datacenter`] — `vm-console`, `do-regions` read; the `vm-*`
///   mutations are not here.
/// * [`crate::ipc::host_ops`] — `gateway-status` reads; `host-power` /
///   `gateway-reboot` / `dr-backup` / `lighthouse-*` mutate.
/// * [`crate::ipc::tofu`] — `tofu-plan` / `tofu-state` read; `tofu-apply` /
///   `tofu-destroy` mutate.
/// * [`crate::ipc::dc_power`] — `wol` mutates (it powers a machine on).
pub const READ_ONLY_VERBS: &[&str] = &[
    "vm-console",
    "do-regions",
    "gateway-status",
    "tofu-plan",
    "tofu-state",
];

/// Whether `verb` mutates state (and so needs `operator`). PURE. Deny-by-default:
/// any verb NOT in [`READ_ONLY_VERBS`] is considered mutating, so a freshly-added
/// action is operator-gated until it is explicitly classified read-only.
#[must_use]
pub fn verb_is_mutating(verb: &str) -> bool {
    !READ_ONLY_VERBS.contains(&verb)
}

/// The principal→role policy. Explicit `entries` win; an unlisted (or absent)
/// principal falls to `default_role`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleMap {
    entries: BTreeMap<String, Role>,
    default_role: Role,
}

impl RoleMap {
    /// Build from explicit entries + a default role (used by tests + parsing).
    #[must_use]
    pub const fn new(entries: BTreeMap<String, Role>, default_role: Role) -> Self {
        Self {
            entries,
            default_role,
        }
    }

    /// The transparent single-operator map: no entries, default `operator`. Every
    /// verb is allowed (the live solo-operator default when no policy is set).
    #[must_use]
    pub const fn open() -> Self {
        Self {
            entries: BTreeMap::new(),
            default_role: Role::Operator,
        }
    }

    /// The role for `principal` (the resolved identity, or `None`/empty when the
    /// request carried no principal). Explicit entry wins; otherwise the default.
    #[must_use]
    pub fn role_for(&self, principal: Option<&str>) -> Role {
        match principal {
            Some(p) if !p.is_empty() => self.entries.get(p).copied().unwrap_or(self.default_role),
            _ => self.default_role,
        }
    }

    /// Parse a policy from the raw env strings. PURE (no env read — the caller
    /// supplies the values, so this is hermetically unit-testable).
    ///
    /// * `roles_raw` — `name=role` pairs separated by `,` or `;` (the role
    ///   separator may be `=` or `:`), e.g. `"alice=operator,bob=viewer"`. A pair
    ///   with an unparseable role, or an empty name, is skipped.
    /// * `default_raw` — the role for an unlisted/absent principal.
    ///
    /// Policy:
    /// * both empty/absent → [`RoleMap::open`] (single-operator mode, transparent).
    /// * `roles_raw` set, no `default_raw` → entries + default `viewer` (a new,
    ///   ungranted principal is read-only by default — fail-closed for mutations).
    /// * `default_raw` set (even with no entries) → entries + that default (lets an
    ///   operator lock the whole plane to `viewer` with `MCNF_DC_DEFAULT_ROLE`).
    #[must_use]
    pub fn parse(roles_raw: &str, default_raw: Option<&str>) -> Self {
        let roles_raw = roles_raw.trim();
        let default_raw = default_raw.map(str::trim).filter(|s| !s.is_empty());

        // No policy configured at all → transparent single-operator mode.
        if roles_raw.is_empty() && default_raw.is_none() {
            return Self::open();
        }

        let mut entries = BTreeMap::new();
        for pair in roles_raw.split([',', ';']) {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let Some((name, role_tok)) = pair.split_once(['=', ':']) else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if let Some(role) = Role::parse(role_tok) {
                entries.insert(name.to_string(), role);
            }
        }

        // A configured policy defaults unlisted principals to `viewer` (read-only)
        // unless an explicit default is given.
        let default_role = default_raw.and_then(Role::parse).unwrap_or(Role::Viewer);

        Self {
            entries,
            default_role,
        }
    }

    /// Load the policy from the environment: `MCNF_DC_ROLES` (the `name=role`
    /// list) + `MCNF_DC_DEFAULT_ROLE` (the unlisted-principal default).
    #[must_use]
    pub fn from_env() -> Self {
        let roles = std::env::var("MCNF_DC_ROLES").unwrap_or_default();
        let default = std::env::var("MCNF_DC_DEFAULT_ROLE").ok();
        Self::parse(&roles, default.as_deref())
    }
}

/// The `principal` string from a request body, if present + non-empty. PURE.
#[must_use]
pub fn principal_from_body(req_body: Option<&str>) -> Option<String> {
    let body = req_body?;
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("principal")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Enforce `map`'s policy for `verb` + the principal carried in `req_body`. PURE.
///
/// `Ok(())` when the resolved role may run the verb; `Err(reason)` (the deny
/// message the responder returns + audits) otherwise. Read-only verbs are always
/// allowed; mutating verbs require `operator`/`admin`.
///
/// # Errors
/// Returns the human-readable deny reason when a non-operator principal attempts
/// a mutating verb.
pub fn enforce_with_map(map: &RoleMap, verb: &str, req_body: Option<&str>) -> Result<(), String> {
    if !verb_is_mutating(verb) {
        return Ok(());
    }
    let principal = principal_from_body(req_body);
    let role = map.role_for(principal.as_deref());
    if role.can_mutate() {
        Ok(())
    } else {
        let who = principal.as_deref().unwrap_or("<unauthenticated>");
        Err(format!(
            "rbac: principal '{who}' (role {role}) may not perform mutating action '{verb}' (operator required)"
        ))
    }
}

/// Enforce the env-configured policy ([`RoleMap::from_env`]) for `verb` +
/// `req_body`. The production entry point the power/provision/tofu responders call
/// before dispatch.
///
/// # Errors
/// As [`enforce_with_map`].
pub fn enforce(verb: &str, req_body: Option<&str>) -> Result<(), String> {
    enforce_with_map(&RoleMap::from_env(), verb, req_body)
}

/// Append a tamper-evident RBAC-denial record to the audit lane
/// (`event/dc/audit/<leaf>`), best-effort (fire-and-reap; a missing `mde-bus`
/// binary is swallowed). Carries the action, the (claimed) principal, the
/// `result:"denied"`, and the reason — so a denied attempt is provable, not just
/// silently refused. Mirrors the other dc workers' Bus-publish lane shape.
pub fn audit_denial(verb: &str, req_body: Option<&str>, reason: &str) {
    let principal =
        principal_from_body(req_body).unwrap_or_else(|| "<unauthenticated>".to_string());
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let topic = format!("event/dc/audit/deny-{millis}-{verb}");
    let body = serde_json::json!({
        "action": format!("dc/{verb}"),
        "principal": principal,
        "result": "denied",
        "reason": reason,
    })
    .to_string();
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- authorize style (MCNF_DC_ROLE_MAP, default-trust) ----

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

    // ---- enforce style (MCNF_DC_ROLES, deny-by-default classifier) ----

    #[test]
    fn read_only_verbs_are_classified_read() {
        for v in READ_ONLY_VERBS {
            assert!(!verb_is_mutating(v), "{v} should be read-only");
        }
    }

    #[test]
    fn mutating_and_unknown_verbs_are_classified_mutating() {
        // Known mutations across all four responders.
        for v in [
            "vm-power",
            "vm-snapshot",
            "vm-clone",
            "vm-delete",
            "host-power",
            "gateway-reboot",
            "dr-backup",
            "dr-ca-backup",
            "dr-rebirth",
            "lighthouse-restart",
            "lighthouse-promote",
            "wol",
            "tofu-apply",
            "tofu-destroy",
            "genesis-new-mesh",
            "testmesh-spin",
            "testmesh-teardown",
            "farm-scale",
        ] {
            assert!(verb_is_mutating(v), "{v} should be mutating");
        }
        // Deny-by-default: an unrecognized verb is treated as a mutation.
        assert!(verb_is_mutating("totally-new-verb"));
    }

    #[test]
    fn role_parse_accepts_aliases_and_rejects_garbage() {
        assert_eq!(Role::parse("viewer"), Some(Role::Viewer));
        assert_eq!(Role::parse("READ"), Some(Role::Viewer));
        assert_eq!(Role::parse(" Operator "), Some(Role::Operator));
        assert_eq!(Role::parse("all"), Some(Role::Operator));
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("superuser"), None);
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn role_can_mutate_matches_the_two_role_model() {
        assert!(!Role::Viewer.can_mutate());
        assert!(Role::Operator.can_mutate());
        assert!(Role::Admin.can_mutate());
    }

    #[test]
    fn parse_empty_is_transparent_single_operator_mode() {
        let m = RoleMap::parse("", None);
        assert_eq!(m, RoleMap::open());
        // Absent / any principal resolves to operator → every verb allowed.
        assert_eq!(m.role_for(None), Role::Operator);
        assert_eq!(m.role_for(Some("anyone")), Role::Operator);
    }

    #[test]
    fn parse_entries_and_default_viewer_when_unspecified() {
        let m = RoleMap::parse("alice=operator,bob=viewer", None);
        assert_eq!(m.role_for(Some("alice")), Role::Operator);
        assert_eq!(m.role_for(Some("bob")), Role::Viewer);
        // An unlisted principal falls to the default (viewer when none given).
        assert_eq!(m.role_for(Some("carol")), Role::Viewer);
        // An absent principal also falls to the default.
        assert_eq!(m.role_for(None), Role::Viewer);
    }

    #[test]
    fn parse_explicit_default_role_locks_the_plane() {
        // No entries, but an explicit default of viewer → everything read-only.
        let m = RoleMap::parse("", Some("viewer"));
        assert_eq!(m.role_for(Some("alice")), Role::Viewer);
        assert_eq!(m.role_for(None), Role::Viewer);
    }

    #[test]
    fn parse_tolerates_separators_and_garbage_pairs() {
        // `;` entry sep, `:` role sep, a garbage pair (skipped), an unknown role
        // (skipped), whitespace.
        let m = RoleMap::parse(
            " alice:operator ; junkpair ; eve:wizard ; bob = viewer ",
            None,
        );
        assert_eq!(m.role_for(Some("alice")), Role::Operator);
        assert_eq!(m.role_for(Some("bob")), Role::Viewer);
        // eve's role was unparseable → not inserted → falls to the default.
        assert_eq!(m.role_for(Some("eve")), Role::Viewer);
    }

    #[test]
    fn principal_from_body_extracts_or_none() {
        assert_eq!(
            principal_from_body(Some(&json!({ "principal": "alice" }).to_string())),
            Some("alice".to_string())
        );
        // Empty principal is treated as absent.
        assert_eq!(
            principal_from_body(Some(&json!({ "principal": "" }).to_string())),
            None
        );
        assert_eq!(
            principal_from_body(Some(&json!({ "uuid": "x" }).to_string())),
            None
        );
        assert_eq!(principal_from_body(Some("not json")), None);
        assert_eq!(principal_from_body(None), None);
    }

    #[test]
    fn enforce_allows_reads_for_everyone_even_viewers() {
        // A viewer-default, no-entries map: read-only verbs still pass.
        let m = RoleMap::parse("", Some("viewer"));
        for v in READ_ONLY_VERBS {
            assert!(
                enforce_with_map(&m, v, Some(&json!({ "principal": "bob" }).to_string())).is_ok(),
                "viewer must be allowed to run read-only {v}"
            );
        }
    }

    #[test]
    fn enforce_denies_viewer_mutation_and_allows_operator() {
        let m = RoleMap::parse("alice=operator,bob=viewer", None);
        // Operator may mutate.
        assert!(enforce_with_map(
            &m,
            "vm-power",
            Some(&json!({ "principal": "alice", "uuid": "abcd-1234", "op": "start" }).to_string())
        )
        .is_ok());
        // Viewer is denied the mutation, with a clear reason naming the principal,
        // role, and verb.
        let denied = enforce_with_map(
            &m,
            "vm-power",
            Some(&json!({ "principal": "bob", "uuid": "abcd-1234", "op": "start" }).to_string()),
        )
        .unwrap_err();
        assert!(denied.contains("rbac:"), "{denied}");
        assert!(denied.contains("bob"), "{denied}");
        assert!(denied.contains("viewer"), "{denied}");
        assert!(denied.contains("vm-power"), "{denied}");
        assert!(denied.contains("operator required"), "{denied}");
    }

    #[test]
    fn enforce_denies_unauthenticated_mutation_under_a_policy() {
        // With a policy configured, a request carrying NO principal falls to the
        // viewer default and is denied any mutation (fail-closed).
        let m = RoleMap::parse("alice=operator", None);
        let denied = enforce_with_map(
            &m,
            "tofu-apply",
            Some(&json!({ "workspace": "xen-xapi", "confirm": true }).to_string()),
        )
        .unwrap_err();
        assert!(denied.contains("<unauthenticated>"), "{denied}");
        assert!(denied.contains("tofu-apply"), "{denied}");
    }

    #[test]
    fn audit_denial_never_panics_even_without_mde_bus() {
        // Fire-and-reap swallows a missing `mde-bus` binary, so the deny-audit
        // side-effect can never wedge or panic a responder thread (pre-RPM dev box
        // / test env). Just assert it returns cleanly.
        audit_denial(
            "vm-delete",
            Some(&json!({ "principal": "bob", "uuid": "x" }).to_string()),
            "rbac: denied",
        );
        // Also exercise the unauthenticated-principal branch.
        audit_denial("tofu-apply", None, "rbac: denied");
    }

    #[test]
    fn enforce_is_transparent_in_single_operator_mode() {
        // The live default (no policy) allows every verb regardless of principal —
        // so the gate never breaks the solo operator.
        let m = RoleMap::open();
        assert!(
            enforce_with_map(&m, "vm-delete", Some(&json!({ "uuid": "x" }).to_string())).is_ok()
        );
        assert!(enforce_with_map(&m, "tofu-destroy", Some(&json!({}).to_string())).is_ok());
        assert!(enforce_with_map(&m, "vm-console", None).is_ok());
    }
}
