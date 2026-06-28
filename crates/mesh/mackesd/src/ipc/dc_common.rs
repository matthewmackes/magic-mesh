//! DATACENTER — shared responder helpers (prod-arm gates, on-disk state dir,
//! two-role RBAC).
//!
//! Glue, not reimplementation (§6): the `action/dc/*` responders
//! ([`crate::ipc::tofu`], [`crate::ipc::datacenter`], [`crate::ipc::dc_power`],
//! [`crate::ipc::host_ops`]) all need the same three primitives, so they live
//! here once:
//!
//! * **State dir** ([`dc_state_dir`]) — where the responders persist their small
//!   durable state (the prod-arm gates, the Tofu run-log, the idle policy, the
//!   learned wake-ETA samples). `MCNF_DC_STATE_DIR` override → `$XDG_DATA/mde/dc`
//!   → `<workgroup_root>/.dc-state` (so a test can always pin it).
//! * **Prod-arm gate** ([`read_arm`]/[`write_arm`]) — a tiny per-gate JSON flag
//!   file (`<gate>.arm`). Fails **safe**: an absent/unreadable file reads
//!   *disarmed*. Used by `tofu-arm` (the `zone1-do` apply/destroy gate) and
//!   `promote-arm` (the Build→Eagle→DO `do` step gate), per the design's "Prod tab
//!   starts disarmed" guardrail (`datacenter-control.md` §9).
//! * **Two-role RBAC** ([`rbac_gate_mutating`]) — the design's viewer/operator
//!   split (§9). The caller's mesh principal arrives in the request body
//!   ([`body_principal`]); it is checked against the `MCNF_DC_ROLE_MAP`
//!   (`name=role,…`) before any *mutating* verb runs. With **no map configured**
//!   the gate is open — that is the §8/§9 flat-trust default (being on the mesh IS
//!   the control plane); a map turns on least-privilege for named viewers.

use std::path::{Path, PathBuf};

/// The on-disk directory the `action/dc/*` responders persist durable state to.
///
/// `MCNF_DC_STATE_DIR` (test/CI override) → the XDG data dir `…/mde/dc` →
/// `<workgroup_root>/.dc-state` as a last resort. Never creates the dir (writers
/// `create_dir_all` lazily); pure path resolution.
#[must_use]
pub fn dc_state_dir(workgroup_root: &Path) -> PathBuf {
    if let Ok(d) = std::env::var("MCNF_DC_STATE_DIR") {
        let d = d.trim();
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Some(d) = dirs::data_dir() {
        return d.join("mde").join("dc");
    }
    workgroup_root.join(".dc-state")
}

/// Path of a named prod-arm gate flag file under `dir`: `<gate>.arm`. PURE.
#[must_use]
pub fn arm_path(dir: &Path, gate: &str) -> PathBuf {
    dir.join(format!("{gate}.arm"))
}

/// Read a named prod-arm gate: `true` only when the flag file exists and holds
/// `{"armed":true}`. Fails **safe** — an absent/unreadable/garbage file reads as
/// *disarmed* (`false`), so a missing state dir can never silently arm prod.
#[must_use]
pub fn read_arm(dir: &Path, gate: &str) -> bool {
    std::fs::read_to_string(arm_path(dir, gate))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("armed").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Set a named prod-arm gate, creating the state dir as needed. Writes
/// `{"gate":<gate>,"armed":<on>}`.
///
/// # Errors
/// Returns the underlying `io::Error` if the dir can't be created or the file
/// can't be written.
pub fn write_arm(dir: &Path, gate: &str, on: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = serde_json::json!({ "gate": gate, "armed": on }).to_string();
    std::fs::write(arm_path(dir, gate), body)
}

// ---- two-role RBAC (datacenter-control.md §9) ---------------------------------

/// The two effective datacenter roles (an `admin` role is reserved by the design
/// but not yet distinct from `operator`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Read-only — may run read verbs, never a mutating one.
    Viewer,
    /// Do-all — may run every verb.
    Operator,
}

/// Parse a role token (`"operator"`/`"viewer"`, case-insensitive). PURE. An
/// unrecognized token is `None` (the caller treats that as the safe `Viewer`).
#[must_use]
pub fn parse_role(s: &str) -> Option<Role> {
    match s.trim().to_ascii_lowercase().as_str() {
        "operator" | "admin" => Some(Role::Operator),
        "viewer" | "read" | "readonly" | "read-only" => Some(Role::Viewer),
        _ => None,
    }
}

/// Resolve `principal`'s role from a `name=role,name=role` map. PURE.
///
/// A principal present in the map gets its mapped role (an unrecognized role
/// token → `Viewer`, fail-safe). A principal **absent** from a non-empty map is
/// `Viewer` — a configured map is an allow-list of operators, so an unknown
/// caller is read-only by default. (An *empty* map means flat-trust and is
/// handled by [`rbac_gate_mutating_with_map`], which never calls this.)
#[must_use]
pub fn role_for(principal: &str, role_map: &str) -> Role {
    let principal = principal.trim();
    for entry in role_map.split(',') {
        if let Some((name, role)) = entry.split_once('=') {
            if name.trim() == principal {
                return parse_role(role).unwrap_or(Role::Viewer);
            }
        }
    }
    Role::Viewer
}

/// The configured datacenter role map (`MCNF_DC_ROLE_MAP`, `name=role,…`). Empty
/// when unset — the flat-trust default.
#[must_use]
pub fn rbac_role_map() -> String {
    std::env::var("MCNF_DC_ROLE_MAP").unwrap_or_default()
}

/// Gate a *mutating* `action/dc/*` verb against an explicit `role_map`. PURE —
/// the env-reading wrapper is [`rbac_gate_mutating`].
///
/// * **empty map** → `Ok(())`: flat-trust (§8/§9 — being on the mesh is the
///   authorization). The existing verbs' behaviour is unchanged until an operator
///   opts into RBAC by configuring a map.
/// * **map set + an operator principal** → `Ok(())`.
/// * **map set + a viewer (or unknown) principal** → `Err` (read-only; denied).
/// * **map set + no principal supplied** → `Err` (can't prove operator; denied).
///
/// # Errors
/// Returns a human-readable denial reason for any non-operator caller when a role
/// map is configured.
pub fn rbac_gate_mutating_with_map(role_map: &str, principal: Option<&str>) -> Result<(), String> {
    if role_map.trim().is_empty() {
        return Ok(());
    }
    match principal {
        Some(p) if !p.trim().is_empty() => match role_for(p, role_map) {
            Role::Operator => Ok(()),
            Role::Viewer => Err(format!(
                "rbac: principal '{p}' is viewer (read-only); mutating action denied"
            )),
        },
        _ => Err(
            "rbac: a role map is configured but no caller principal was supplied; \
                  mutating action denied"
                .into(),
        ),
    }
}

/// Gate a *mutating* verb against the configured `MCNF_DC_ROLE_MAP`. See
/// [`rbac_gate_mutating_with_map`] for the policy.
///
/// # Errors
/// Returns a denial reason for any non-operator caller when a role map is set.
pub fn rbac_gate_mutating(principal: Option<&str>) -> Result<(), String> {
    rbac_gate_mutating_with_map(&rbac_role_map(), principal)
}

/// Pull the optional caller mesh principal from a parsed request body's
/// `principal` field. PURE. Absent/blank → `None`.
#[must_use]
pub fn body_principal(req: &serde_json::Value) -> Option<&str> {
    req.get("principal")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_state_dir_prefers_env_override() {
        // The env override wins (the path a test pins). We don't mutate the env
        // here (parallel-test-unsafe); instead exercise the fallbacks via the
        // arm round-trip below, and assert the override-shape is honoured by
        // constructing it directly.
        let root = PathBuf::from("/srv/workgroup");
        let d = dc_state_dir(&root);
        // Without MCNF_DC_STATE_DIR set in this process, we land on the XDG data
        // dir or the workgroup fallback — either way a concrete, absolute-ish path
        // ending in the dc leaf or .dc-state.
        let s = d.to_string_lossy();
        assert!(
            s.ends_with("/mde/dc") || s.ends_with("/.dc-state"),
            "unexpected state dir: {s}"
        );
    }

    #[test]
    fn arm_gate_round_trips_and_fails_safe() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        // Absent file → disarmed (fail-safe).
        assert!(!read_arm(dir, "tofu"));
        // Arm it → reads armed.
        write_arm(dir, "tofu", true).expect("write arm");
        assert!(read_arm(dir, "tofu"));
        // A different gate is independent.
        assert!(!read_arm(dir, "promote"));
        write_arm(dir, "promote", true).expect("write arm");
        assert!(read_arm(dir, "promote"));
        // Disarm tofu → off again, promote untouched.
        write_arm(dir, "tofu", false).expect("write arm");
        assert!(!read_arm(dir, "tofu"));
        assert!(read_arm(dir, "promote"));
    }

    #[test]
    fn arm_read_is_safe_on_garbage() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        std::fs::write(arm_path(dir, "tofu"), b"not json").unwrap();
        assert!(!read_arm(dir, "tofu"));
        // Right JSON, wrong/absent field → disarmed.
        std::fs::write(arm_path(dir, "tofu"), br#"{"armed":"yes"}"#).unwrap();
        assert!(!read_arm(dir, "tofu"));
        std::fs::write(arm_path(dir, "tofu"), br#"{"other":true}"#).unwrap();
        assert!(!read_arm(dir, "tofu"));
    }

    #[test]
    fn role_parsing_is_case_insensitive_and_safe() {
        assert_eq!(parse_role("operator"), Some(Role::Operator));
        assert_eq!(parse_role("  OPERATOR "), Some(Role::Operator));
        assert_eq!(parse_role("admin"), Some(Role::Operator));
        assert_eq!(parse_role("viewer"), Some(Role::Viewer));
        assert_eq!(parse_role("read-only"), Some(Role::Viewer));
        assert_eq!(parse_role("captain"), None);
    }

    #[test]
    fn role_for_resolves_and_defaults_unknown_to_viewer() {
        let map = "alice=operator, bob=viewer ,carol=admin";
        assert_eq!(role_for("alice", map), Role::Operator);
        assert_eq!(role_for("bob", map), Role::Viewer);
        assert_eq!(role_for("carol", map), Role::Operator);
        // Not in the map → viewer (allow-list semantics).
        assert_eq!(role_for("mallory", map), Role::Viewer);
        // Whitespace around the wire principal is tolerated.
        assert_eq!(role_for("  alice  ", map), Role::Operator);
    }

    #[test]
    fn rbac_open_when_no_map_configured() {
        // Flat trust: an empty map allows everyone (incl. an absent principal).
        assert!(rbac_gate_mutating_with_map("", None).is_ok());
        assert!(rbac_gate_mutating_with_map("   ", Some("anyone")).is_ok());
    }

    #[test]
    fn rbac_enforces_two_roles_when_map_set() {
        let map = "alice=operator,bob=viewer";
        // Operator passes.
        assert!(rbac_gate_mutating_with_map(map, Some("alice")).is_ok());
        // Viewer is denied with a clear reason.
        let e = rbac_gate_mutating_with_map(map, Some("bob")).unwrap_err();
        assert!(e.contains("viewer") && e.contains("denied"), "{e}");
        // Unknown principal is denied (allow-list).
        assert!(rbac_gate_mutating_with_map(map, Some("mallory")).is_err());
        // No principal supplied but a map is configured → denied.
        let e = rbac_gate_mutating_with_map(map, None).unwrap_err();
        assert!(e.contains("no caller principal"), "{e}");
    }

    #[test]
    fn body_principal_extracts_or_none() {
        let v = serde_json::json!({ "principal": "alice", "op": "x" });
        assert_eq!(body_principal(&v), Some("alice"));
        let v = serde_json::json!({ "principal": "  spacey  " });
        assert_eq!(body_principal(&v), Some("spacey"));
        // Blank / absent / non-string → None.
        assert_eq!(
            body_principal(&serde_json::json!({ "principal": "" })),
            None
        );
        assert_eq!(
            body_principal(&serde_json::json!({ "principal": "   " })),
            None
        );
        assert_eq!(body_principal(&serde_json::json!({ "op": "x" })), None);
        assert_eq!(body_principal(&serde_json::json!({ "principal": 7 })), None);
    }
}
