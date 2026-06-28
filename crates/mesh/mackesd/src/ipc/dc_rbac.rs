//! DATACENTER-7 (RBAC half) — the role gate every `action/dc/*` responder runs
//! BEFORE dispatching a mutating verb.
//!
//! The Datacenter plane ships **two effective roles** (design
//! `docs/design/datacenter-control.md` §9, RBAC two-role lock): **viewer**
//! (read-only) and **operator** (do-all), on a framework with **admin**
//! reserved. The *principal* is the caller's mesh identity — its Nebula
//! cert/peer name. The Bus carries no native sender field (it is a flat,
//! mesh-authenticated pub/sub: being a trusted enrolled node is the
//! authentication — AI_GOVERNANCE §8 open-mesh envelope), so the GUI carries the
//! principal in the request body's `principal` field; this gate maps it to a
//! [`Role`] via a [`RoleMap`] and enforces:
//!
//! * a **mutating** verb (everything not in [`READ_ONLY_VERBS`]) requires
//!   `operator` (or the reserved `admin`); a `viewer` is **denied**;
//! * a **read-only** verb is allowed for any resolved role.
//!
//! The map is configured out-of-band (env, [`RoleMap::from_env`]). When **no**
//! policy is configured the plane runs in **single-operator mode** (the live
//! solo-operator default, §8 small-trusted-workgroup): the default role is
//! `operator`, so every verb is allowed and the gate is transparent. The moment
//! an operator sets `MCNF_DC_ROLES`, the gate enforces — unlisted principals fall
//! to `MCNF_DC_DEFAULT_ROLE` (default `viewer`, i.e. read-only), so adding a
//! second principal automatically locks it out of mutations until it is granted
//! `operator`.
//!
//! On a denial the caller [`enforce`] returns `Err(reason)`; the responder turns
//! that into the `{"error":…}` envelope AND appends a tamper-evident
//! `event/dc/audit/*` deny record ([`audit_denial`]) — "deny + audit on failure".
//!
//! The classifier ([`verb_is_mutating`]) is deny-by-default: an **unknown** verb
//! is treated as mutating, so a newly-added action verb is operator-gated until
//! someone deliberately marks it read-only.

use std::collections::BTreeMap;

/// The read-only `action/dc/*` verbs — allowed for any resolved role (a viewer
/// may read). Everything NOT in this set is treated as a mutation and requires
/// `operator`/`admin` (deny-by-default for unknown verbs).
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

/// A Datacenter-plane role. Two ship (`Viewer`, `Operator`); `Admin` is the
/// reserved third tier on the framework (today it grants the same as `Operator`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Read-only: may run the [`READ_ONLY_VERBS`], denied every mutation.
    Viewer,
    /// Do-all: may run every verb.
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
/// `req_body`. The production entry point the responders call before dispatch.
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
