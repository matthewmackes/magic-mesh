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
//!     "display_name"?,"expires"?}`; writes gateway.toml. Empty `host` clears it.
//!   * `get-gateway`   — no body; reply `{"present":bool, ...fields}` for the panel.
//!   * `clear-gateway` — no body; removes gateway.toml (reverts every node to P2P).
//!
//! The password travels only over the per-node tmpfs Bus + lands in a 0600 file;
//! it is never passed on a command line (absent from `ps`). At-rest age-encryption
//! on QNM-Shared is a noted hardening follow-on (no age helper exists yet).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// Responder handle — carries the QNM-Shared root the gateway file lives under.
#[derive(Debug, Clone)]
pub struct VoipService {
    workgroup_root: PathBuf,
}

impl VoipService {
    /// New service writing under `workgroup_root` (the QNM-Shared mount).
    #[must_use]
    pub fn new(workgroup_root: &Path) -> Self {
        Self {
            workgroup_root: workgroup_root.to_path_buf(),
        }
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
            // Empty host = clear (revert the mesh to P2P).
            if host.is_empty() {
                let _ = std::fs::remove_file(&path);
                return json!({ "ok": true, "cleared": true }).to_string();
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
            let rec = GatewayFile {
                username,
                password: req
                    .get("password")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
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
                    "password": rec.password,
                    "display_name": rec.display_name,
                    "expires": rec.expires,
                })
                .to_string()
            }
            None => json!({ "present": false }).to_string(),
        },
        "clear-gateway" => {
            let _ = std::fs::remove_file(&path);
            json!({ "ok": true }).to_string()
        }
        other => err(format!("voip: unknown verb {other}")),
    }
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
                build_reply(svc, verb, msg.body.as_deref())
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
}
