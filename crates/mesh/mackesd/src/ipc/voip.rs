//! Mesh-wide SIP outbound gateway, served on the **Bus** at `action/voip/<verb>`
//! (VOIP-GW-1).
//!
//! The operator sets ONE outbound SIP/PSTN gateway in the Workbench; this
//! responder (running as the root daemon, the only writer with access to the
//! QNM-Shared mount) persists it to `<workgroup_root>/voip/gateway.toml` in the
//! exact `account.toml` shape the voice agent already consumes (VOIP-P2P-4). The
//! file replicates over QNM-Shared, so every node's `mde-voice-hud` agent reads
//! the same gateway and registers to it — bare numbers route out via the gateway
//! while intra-mesh peer calls stay P2P.
//!
//! Verbs (args in the request body):
//!   * `set-gateway`   — body `{"host","port"?,"username","password"?,
//!     "display_name"?,"expires"?}`; writes gateway.toml. Empty `host` clears a
//!     present gateway; a second empty-host clear refuses. Malformed hosts
//!     (scheme, path, whitespace, embedded port) refuse before any write.
//!   * `get-gateway`   — no body; reply `{"present":bool, ...fields}` for the
//!     panel. The stored password is never in the reply (`password` is `""`;
//!     `password_set` reports whether a secret is stored).
//!   * `clear-gateway` — an authenticated JSON envelope (payload ignored);
//!     removes gateway.toml (reverts every node to P2P). A clear when already
//!     absent, or a replayed armed token, refuses.
//!
//! The password travels only over the per-node tmpfs Bus + lands in a 0600 file;
//! it is never passed on a command line (absent from `ps`) and never rendered
//! in a Bus reply or `Debug`. At-rest age-encryption on QNM-Shared is a noted
//! hardening follow-on (no age helper exists yet).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// Responder handle — carries the QNM-Shared root the gateway file lives under.
#[derive(Debug, Clone)]
pub struct VoipService {
    workgroup_root: PathBuf,
    authorizer: Arc<ActionAuthorizer>,
}

impl VoipService {
    /// New service writing under `workgroup_root` (the QNM-Shared mount).
    #[must_use]
    pub fn new(workgroup_root: &Path) -> Self {
        Self {
            workgroup_root: workgroup_root.to_path_buf(),
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject an isolated verifier and replay ledger for hostile responder
    /// tests. Production always uses [`ActionAuthorizer::production`].
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }
}

/// Action verbs served on `action/voip/<verb>`.
pub const ACTION_VERBS: [&str; 3] = ["set-gateway", "get-gateway", "clear-gateway"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// `action/voip/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/voip/{verb}")
}

/// The shared gateway file, in the voice agent's `account.toml` shape.
#[must_use]
pub fn gateway_path(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("voip").join("gateway.toml")
}

/// On-disk gateway record — identical fields to the voice agent's `AccountFile`
/// so `mde-voice-hud` parses it with no translation.
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct GatewayFile {
    username: String,
    #[serde(default)]
    password: String,
    /// Registrar as `host` or `host:port`.
    server: String,
    #[serde(default)]
    display_name: String,
    #[serde(default = "default_expires")]
    expires: u32,
}

impl std::fmt::Debug for GatewayFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayFile")
            .field("username", &self.username)
            .field(
                "password",
                if self.password.is_empty() {
                    &""
                } else {
                    &"<redacted>"
                },
            )
            .field("server", &self.server)
            .field("display_name", &self.display_name)
            .field("expires", &self.expires)
            .finish()
    }
}

fn default_expires() -> u32 {
    3600
}

/// Build the reply body for one `action/voip/<verb>` request.
#[must_use]
pub fn build_reply(svc: &VoipService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let path = gateway_path(&svc.workgroup_root);
    match verb {
        "set-gateway" => {
            let Some(body) = req_body else {
                return err("set-gateway: missing request body".into());
            };
            let req: serde_json::Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => return err(format!("set-gateway: bad json: {e}")),
            };
            let host = req
                .get("host")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            // Empty host = clear a *present* gateway (revert the mesh to P2P).
            // A second empty-host clear is a replay and refuses; Activity treats
            // empty host as malformed at the UI, so this shortcut is daemon-only.
            if host.is_empty() {
                return clear_gateway(&path, "set-gateway");
            }
            if !is_valid_gateway_host(&host) {
                return err("set-gateway: malformed host".into());
            }
            let username = req
                .get("username")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if username.is_empty() {
                return err("set-gateway: username is required".into());
            }
            let port = req.get("port").and_then(serde_json::Value::as_u64);
            let server = match port {
                Some(p) if p > 0 && p != 5060 => format!("{host}:{p}"),
                _ => host,
            };
            // `get-gateway` deliberately redacts the password. Treat a blank
            // password posted back by the existing panel as "unchanged" when
            // a credential is already stored, so loading and applying the
            // form cannot silently erase the gateway secret. A new gateway
            // may still be created without a password; clearing the whole
            // gateway remains the explicit way to remove the record.
            let requested_password = req
                .get("password")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let password = if requested_password.is_empty() {
                read_gateway(&path)
                    .map(|current| current.password)
                    .unwrap_or_default()
            } else {
                requested_password.to_string()
            };
            let rec = GatewayFile {
                username,
                password,
                server,
                display_name: req
                    .get("display_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                expires: req
                    .get("expires")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or_else(default_expires),
            };
            match write_gateway(&path, &rec) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("set-gateway: {e}")),
            }
        }
        "get-gateway" => match read_gateway(&path) {
            Some(rec) => {
                let (host, port) = split_host_port(&rec.server);
                json!({
                    "present": true,
                    "host": host,
                    "port": port,
                    "username": rec.username,
                    // Keep the panel's password field in the response shape,
                    // but never return the stored credential over the open
                    // read. `password_set` lets a caller distinguish a
                    // configured secret from an intentionally empty one.
                    "password": "",
                    "password_set": !rec.password.is_empty(),
                    "display_name": rec.display_name,
                    "expires": rec.expires,
                })
                .to_string()
            }
            None => json!({ "present": false }).to_string(),
        },
        "clear-gateway" => clear_gateway(&path, "clear-gateway"),
        other => err(format!("voip: unknown verb {other}")),
    }
}

/// Stable consumer scope for VoIP gateway mutations. The gateway is one
/// replicated workgroup setting, so callers cannot select a different node or
/// path through the capability target.
const VOIP_ACTION_NODE_SCOPE: &str = "voip";

/// Parse a privileged VoIP mutation into its stable capability target and the
/// legacy body consumed by [`build_reply`]. Authentication metadata is removed
/// only after the exact original body has been verified; this helper performs
/// no filesystem reads or writes.
fn mutation_request(
    verb: &str,
    req_body: Option<&str>,
) -> Result<Option<(String, String)>, String> {
    if !matches!(verb, "set-gateway" | "clear-gateway") {
        return Ok(None);
    }
    let body = req_body.ok_or_else(|| format!("{verb}: missing request body"))?;
    let mut request: serde_json::Value =
        serde_json::from_str(body).map_err(|_| format!("{verb}: request body must be JSON"))?;
    let object = request
        .as_object_mut()
        .ok_or_else(|| format!("{verb}: request body must be a JSON object"))?;
    object.remove("schema_version");
    object.remove("armed_token");

    // Both mutations operate on the same singleton gateway record. Keeping a
    // closed target prevents a caller from minting a capability for an
    // arbitrary host or path.
    let handler_body = if verb == "clear-gateway" {
        // `build_reply` ignores the clear body, but retain a valid JSON object
        // so the authenticated request shape is deterministic.
        serde_json::Value::Object(object.clone()).to_string()
    } else {
        request.to_string()
    };
    Ok(Some(("gateway".to_string(), handler_body)))
}

/// Apply the shared-Bus authorization boundary before a VoIP mutation can
/// touch the replicated gateway file. `get-gateway` remains read-only and
/// unauthenticated.
fn build_bus_reply(svc: &VoipService, verb: &str, req_body: Option<&str>) -> String {
    let prepared = match mutation_request(verb, req_body) {
        Ok(prepared) => prepared,
        Err(error) => return json!({ "error": error }).to_string(),
    };
    let Some((target, handler_body)) = prepared else {
        return build_reply(svc, verb, req_body);
    };

    let auth_verb = format!("voip-{verb}");
    let context = MutationContext {
        verb: &auth_verb,
        node: VOIP_ACTION_NODE_SCOPE,
        target: &target,
    };
    if let Err(error) = svc
        .authorizer
        .authorize(req_body.expect("a mutation requires a body"), context)
    {
        tracing::warn!(
            target: "mackesd::voip",
            verb,
            %error,
            "refused unauthorized VoIP mutation"
        );
        return json!({ "error": format!("{verb}: authorization refused: {error}") }).to_string();
    }
    build_reply(svc, verb, Some(&handler_body))
}

/// Remove a present `gateway.toml`. Absent file is a replayed clear and refuses.
fn clear_gateway(path: &Path, verb: &str) -> String {
    if !path.exists() {
        return json!({ "error": format!("{verb}: gateway is already cleared") }).to_string();
    }
    match std::fs::remove_file(path) {
        Ok(()) => {
            if verb == "set-gateway" {
                json!({ "ok": true, "cleared": true }).to_string()
            } else {
                json!({ "ok": true }).to_string()
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            json!({ "error": format!("{verb}: gateway is already cleared") }).to_string()
        }
        Err(e) => json!({ "error": format!("{verb}: {e}") }).to_string(),
    }
}

/// Registrar host: IPv4 or DNS label, no scheme, path, port, or whitespace.
/// Mirrors `mde-collab-egui` `is_valid_gateway_host` so a Bus caller cannot
/// bypass the Activity refuse by posting `set-gateway` directly.
fn is_valid_gateway_host(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.contains("://")
        || host.contains('/')
        || host.contains('\\')
        || host.contains('@')
        || host.contains(' ')
        || host.contains(':')
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return false;
    }
    if is_ipv4_host(host) {
        return true;
    }
    host.split('.').all(is_dns_label)
}

fn is_ipv4_host(host: &str) -> bool {
    let mut count = 0usize;
    for part in host.split('.') {
        count += 1;
        if count > 4 {
            return false;
        }
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if part.parse::<u8>().is_err() {
            return false;
        }
    }
    count == 4
}

fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// Write the gateway file atomically with 0600 perms (the password is in it).
fn write_gateway(path: &Path, rec: &GatewayFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml = toml::to_string(rec).map_err(|e| std::io::Error::other(e.to_string()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml.as_bytes())?;
    set_owner_only(&tmp);
    std::fs::rename(&tmp, path)?;
    set_owner_only(path);
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

fn read_gateway(path: &Path) -> Option<GatewayFile> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// Split `host` / `host:port`, defaulting 5060 (mirrors the voice agent).
fn split_host_port(server: &str) -> (String, u16) {
    match server.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (server.to_string(), 5060),
        },
        _ => (server.to_string(), 5060),
    }
}

/// Run the responder loop until `should_stop` (mirrors the settings responder).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &VoipService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out so a test can drive it).
pub fn poll_once(persist: &Persist, svc: &VoipService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "voip responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_bus_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "voip responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::authorize_test_body;

    const AUTH_KEY: &[u8] = b"voip-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    #[test]
    fn set_then_get_round_trips_and_writes_account_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let body = json!({
            "host": "pbx.example.com", "port": 5062, "username": "alice",
            "password": "s3cret", "display_name": "Alice"
        })
        .to_string();
        let r = build_reply(&svc, "set-gateway", Some(&body));
        assert!(r.contains("\"ok\":true"), "{r}");

        // The file is valid TOML in the voice agent's account.toml shape.
        let written = std::fs::read_to_string(gateway_path(tmp.path())).unwrap();
        assert!(
            written.contains("server = \"pbx.example.com:5062\""),
            "{written}"
        );
        assert!(written.contains("username = \"alice\""));

        // get-gateway returns the fields (host/port split back out).
        let g = build_reply(&svc, "get-gateway", None);
        let v: serde_json::Value = serde_json::from_str(&g).unwrap();
        assert_eq!(v["present"], true);
        assert_eq!(v["host"], "pbx.example.com");
        assert_eq!(v["port"], 5062);
        assert_eq!(v["username"], "alice");
        assert_eq!(v["password"], "");
        assert_eq!(v["password_set"], true);
        assert!(
            !g.contains("s3cret"),
            "get-gateway leaked the password: {g}"
        );
    }

    #[test]
    fn redacted_get_can_be_resubmitted_without_clearing_password() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let initial = json!({
            "host": "pbx.example.com",
            "username": "alice",
            "password": "s3cret",
            "display_name": "Alice"
        })
        .to_string();
        assert!(build_reply(&svc, "set-gateway", Some(&initial)).contains("\"ok\":true"));

        let loaded: serde_json::Value =
            serde_json::from_str(&build_reply(&svc, "get-gateway", None)).unwrap();
        let resubmitted = json!({
            "host": loaded["host"],
            "port": loaded["port"],
            "username": loaded["username"],
            "password": loaded["password"],
            "display_name": loaded["display_name"]
        })
        .to_string();
        assert!(build_reply(&svc, "set-gateway", Some(&resubmitted)).contains("\"ok\":true"));

        let stored = read_gateway(&gateway_path(tmp.path())).expect("gateway remains present");
        assert_eq!(stored.password, "s3cret");
    }

    #[test]
    fn empty_host_clears_the_gateway() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let _ = build_reply(
            &svc,
            "set-gateway",
            Some(&json!({"host":"h","username":"u"}).to_string()),
        );
        assert!(gateway_path(tmp.path()).exists());
        let r = build_reply(&svc, "set-gateway", Some(&json!({"host":""}).to_string()));
        assert!(r.contains("cleared"), "{r}");
        assert!(!gateway_path(tmp.path()).exists());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&build_reply(&svc, "get-gateway", None))
                .unwrap()["present"],
            false
        );
    }

    #[test]
    fn default_port_5060_is_not_appended() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let _ = build_reply(
            &svc,
            "set-gateway",
            Some(&json!({"host":"h","port":5060,"username":"u"}).to_string()),
        );
        let written = std::fs::read_to_string(gateway_path(tmp.path())).unwrap();
        assert!(written.contains("server = \"h\""), "{written}");
    }

    #[test]
    fn hostile_unsigned_mutations_are_refused_before_gateway_io() {
        let (tmp, svc) = {
            let tmp = tempfile::tempdir().unwrap();
            let svc = VoipService::new(tmp.path()).with_authorizer(Arc::new(
                ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW),
            ));
            (tmp, svc)
        };
        let set = json!({
            "schema_version": 1,
            "host": "pbx.example.com",
            "username": "unsigned"
        })
        .to_string();
        let clear = json!({ "schema_version": 1 }).to_string();

        for (verb, body) in [("set-gateway", set), ("clear-gateway", clear)] {
            let reply = build_bus_reply(&svc, verb, Some(&body));
            assert!(
                reply.contains("authorization refused"),
                "unsigned {verb} reached its handler: {reply}"
            );
        }
        assert!(!gateway_path(tmp.path()).exists());
    }

    #[test]
    fn authorized_gateway_mutation_is_exact_body_bound_and_single_use() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path()).with_authorizer(Arc::new(
            ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW),
        ));
        let unsigned = json!({
            "schema_version": 1,
            "host": "pbx.example.com",
            "port": 5062,
            "username": "authorized",
            "password": "s3cret",
            "display_name": "Authorized"
        })
        .to_string();
        let context = MutationContext {
            verb: "voip-set-gateway",
            node: VOIP_ACTION_NODE_SCOPE,
            target: "gateway",
        };
        let armed = authorize_test_body(
            AUTH_KEY,
            &unsigned,
            context,
            "voip-replay",
            AUTH_NOW + 30_000,
        );

        let first = build_bus_reply(&svc, "set-gateway", Some(&armed));
        assert!(first.contains("\"ok\":true"), "{first}");
        let replay = build_bus_reply(&svc, "set-gateway", Some(&armed));
        assert!(replay.contains("already used"), "{replay}");

        let tampered = armed.replace(
            "\"host\":\"pbx.example.com\"",
            "\"host\":\"evil.example.com\"",
        );
        assert!(
            build_bus_reply(&svc, "set-gateway", Some(&tampered)).contains("authorization refused")
        );
        let written = std::fs::read_to_string(gateway_path(tmp.path())).unwrap();
        assert!(written.contains("server = \"pbx.example.com:5062\""));
        assert_no_secret(&first, "s3cret");
        assert_no_secret(&replay, "s3cret");

        let clear_unsigned = json!({ "schema_version": 1 }).to_string();
        let clear_context = MutationContext {
            verb: "voip-clear-gateway",
            node: VOIP_ACTION_NODE_SCOPE,
            target: "gateway",
        };
        let clear_armed = authorize_test_body(
            AUTH_KEY,
            &clear_unsigned,
            clear_context,
            "voip-clear",
            AUTH_NOW + 30_000,
        );
        let cleared = build_bus_reply(&svc, "clear-gateway", Some(&clear_armed));
        assert!(cleared.contains("\"ok\":true"), "{cleared}");
        assert!(!gateway_path(tmp.path()).exists());
        let replay_clear = build_bus_reply(&svc, "clear-gateway", Some(&clear_armed));
        assert!(
            replay_clear.contains("already used"),
            "replayed armed clear must refuse: {replay_clear}"
        );
        assert_no_secret(&cleared, "s3cret");
        assert_no_secret(&replay_clear, "s3cret");
    }

    #[test]
    fn malformed_hosts_refuse_before_gateway_io() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let _ = build_reply(
            &svc,
            "set-gateway",
            Some(
                &json!({"host":"pbx.example.com","username":"alice","password":"s3cret"})
                    .to_string(),
            ),
        );
        let before = std::fs::read_to_string(gateway_path(tmp.path())).unwrap();

        for host in [
            "http://pbx.example.com",
            "pbx.example.com/sip",
            "not a host",
            "pbx.example.com:5062",
            "alice@pbx.example.com",
            ".pbx.example.com",
            "pbx.example.com.",
            "pbx..example.com",
        ] {
            let body = json!({
                "host": host,
                "username": "alice",
                "password": "s3cret"
            })
            .to_string();
            let reply = build_reply(&svc, "set-gateway", Some(&body));
            assert!(
                reply.contains("malformed host"),
                "host {host:?} must refuse: {reply}"
            );
            assert_no_secret(&reply, "s3cret");
        }

        let after = std::fs::read_to_string(gateway_path(tmp.path())).unwrap();
        assert_eq!(
            before, after,
            "malformed host must not rewrite gateway.toml"
        );
    }

    #[test]
    fn replayed_clears_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let path = gateway_path(tmp.path());

        let absent_clear = build_reply(&svc, "clear-gateway", Some("{}"));
        assert!(
            absent_clear.contains("already cleared"),
            "clear of an absent gateway must refuse: {absent_clear}"
        );
        assert!(!path.exists());

        let empty_host_absent =
            build_reply(&svc, "set-gateway", Some(&json!({"host":""}).to_string()));
        assert!(
            empty_host_absent.contains("already cleared"),
            "empty-host clear of an absent gateway must refuse: {empty_host_absent}"
        );

        let set = build_reply(
            &svc,
            "set-gateway",
            Some(
                &json!({
                    "host": "pbx.example.com",
                    "username": "alice",
                    "password": "s3cret"
                })
                .to_string(),
            ),
        );
        assert!(set.contains("\"ok\":true"), "{set}");
        assert!(path.exists());

        let first = build_reply(&svc, "clear-gateway", Some("{}"));
        assert!(first.contains("\"ok\":true"), "{first}");
        assert!(!path.exists());

        let second = build_reply(&svc, "clear-gateway", Some("{}"));
        assert!(
            second.contains("already cleared"),
            "second clear must refuse: {second}"
        );
        assert_no_secret(&first, "s3cret");
        assert_no_secret(&second, "s3cret");
        assert_no_secret(&absent_clear, "s3cret");
    }

    #[test]
    fn password_never_renders_in_replies_or_debug() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = VoipService::new(tmp.path());
        let secret = "s3cret-never-render";
        let set = build_reply(
            &svc,
            "set-gateway",
            Some(
                &json!({
                    "host": "pbx.example.com",
                    "port": 5062,
                    "username": "alice",
                    "password": secret,
                    "display_name": "Alice"
                })
                .to_string(),
            ),
        );
        assert!(set.contains("\"ok\":true"), "{set}");
        assert_no_secret(&set, secret);

        let get = build_reply(&svc, "get-gateway", None);
        assert_no_secret(&get, secret);
        let v: serde_json::Value = serde_json::from_str(&get).unwrap();
        assert_eq!(v["password"], "");
        assert_eq!(v["password_set"], true);

        let stored = read_gateway(&gateway_path(tmp.path())).expect("gateway present");
        assert_eq!(stored.password, secret);
        let debug = format!("{stored:?}");
        assert!(
            !debug.contains(secret),
            "GatewayFile Debug leaked the password: {debug}"
        );
        assert!(debug.contains("<redacted>"), "{debug}");

        let clear = build_reply(&svc, "clear-gateway", Some("{}"));
        assert!(clear.contains("\"ok\":true"), "{clear}");
        assert_no_secret(&clear, secret);
        let get_absent = build_reply(&svc, "get-gateway", None);
        assert_no_secret(&get_absent, secret);
        assert!(!get_absent.contains("\"password\""), "{get_absent}");
    }

    /// Replies must never carry the stored/posted secret, including a present
    /// `password` JSON field (that field is allowed only as `""`).
    fn assert_no_secret(reply: &str, secret: &str) {
        assert!(
            !reply.contains(secret),
            "reply leaked the gateway password: {reply}"
        );
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(reply) {
            if let Some(password) = value.get("password") {
                assert_eq!(
                    password.as_str(),
                    Some(""),
                    "password field must be empty: {reply}"
                );
            }
        }
    }
}
