//! File-transfer surfaces — migrated from the
//! `dev.mackes.MDE.Shell.{Inbox,Outbox,Downloads,FileOperations}` +
//! `dev.mackes.MDE.Fleet.Files` D-Bus interfaces onto the mesh **Bus**
//! (E0.3.2). Each surface answers `action/<prefix>/<verb>`; verb
//! arguments (where a verb takes any) travel in the request body, and
//! every reply is a JSON value or an `{"error":…}` envelope.
//!
//! Per the operator's "migrate all to the Bus" disposition: the four
//! `Shell.*` surfaces were never registered on D-Bus and have no live
//! consumer yet, so they ship as Bus responders returning their honest
//! empty / "transport not configured" states — a future epic fills the
//! real transfer engine (the `mackesd::orchestrator` Send-To state
//! machine) and wires consumers. **Fleet.Files is the live one**:
//! mde-files's mesh-browse reads its peer roster from the SQLite
//! `nodes` table.
//!
//! Responders are synchronous (no tokio runtime) and run on dedicated
//! OS threads spawned by mackesd `run_serve` — `Persist`/rusqlite
//! isn't `Send`. Fleet.Files locks its shared store via
//! `tokio::sync::Mutex::blocking_lock`, which is correct on a
//! non-async thread (it would panic inside a runtime).

#![cfg(feature = "async-services")]

use std::collections::HashMap;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// Responder poll interval (shared across the file surfaces).
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// JSON `{"error": <msg>}` envelope — the Bus equivalent of the old
/// `zbus::fdo::Error::Failed`. Callers parse-and-surface rather than
/// time out.
fn err(m: impl Into<String>) -> String {
    json!({ "error": m.into() }).to_string()
}

/// A boxed reply builder: `(verb, body) -> reply-json`. `Send` so it
/// can ride into the responder thread.
pub type ReplyFn = Box<dyn Fn(&str, Option<&str>) -> String + Send>;

/// One file surface registered on the combined responder: its
/// action-topic prefix, its verbs, and its reply builder.
pub struct Surface {
    /// Action-topic prefix, e.g. `fleet-files` (topics are
    /// `action/<prefix>/<verb>`).
    pub prefix: &'static str,
    /// Verbs served under `prefix`.
    pub verbs: &'static [&'static str],
    /// Builds the reply body for `(verb, request-body)`.
    pub reply: ReplyFn,
}

/// Drive ALL the file surfaces from one thread + one `Persist` (cheaper
/// than a thread per surface, and `Persist`/rusqlite isn't `Send` so it
/// can't be shared across threads anyway). Each tick polls every
/// surface's verbs for new requests and writes their replies, until
/// `should_stop()`. mackesd `run_serve` spawns this on a dedicated OS
/// thread.
pub fn serve_all(persist: &Persist, surfaces: &[Surface], should_stop: impl Fn() -> bool) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        for s in surfaces {
            poll_once(persist, s.prefix, s.verbs, &s.reply, &mut cursors);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across `verbs` (split out so a test can drive it
/// without the sleep loop).
fn poll_once<F>(
    persist: &Persist,
    prefix: &str,
    verbs: &[&str],
    reply: &F,
    cursors: &mut HashMap<String, String>,
) where
    F: Fn(&str, Option<&str>) -> String,
{
    for &verb in verbs {
        let topic = format!("action/{prefix}/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "files responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = reply(verb, msg.body.as_deref());
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&body),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "files responder: reply write failed");
            }
        }
    }
}

// ---- Inbox — action/files-inbox/<verb> ----------------------------

/// Action-topic prefix for the inbox surface.
pub const INBOX_PREFIX: &str = "files-inbox";
/// Verbs served on `action/files-inbox/<verb>`.
pub const INBOX_VERBS: [&str; 2] = ["list", "mark-opened"];

/// Reply builder for the inbox surface. `list` is the honest empty
/// state (mesh inbox is the `send_to` destination; AF-5 wires the
/// producer); `mark-opened` has nothing to mark yet.
#[must_use]
pub fn inbox_reply(verb: &str, _body: Option<&str>) -> String {
    match verb {
        "list" => "[]".to_string(),
        "mark-opened" => err("no inbox entries to mark — AF-5 wires the producer side"),
        other => err(format!("unknown inbox verb: {other}")),
    }
}

// ---- Outbox — action/files-outbox/<verb> --------------------------

/// Action-topic prefix for the outbox surface.
pub const OUTBOX_PREFIX: &str = "files-outbox";
/// Verbs served on `action/files-outbox/<verb>`.
pub const OUTBOX_VERBS: [&str; 2] = ["list", "cancel"];

/// Reply builder for the outbox surface. `list` is honest empty;
/// `cancel` (body = op id) has no in-flight upload to cancel yet.
#[must_use]
pub fn outbox_reply(verb: &str, _body: Option<&str>) -> String {
    match verb {
        "list" => "[]".to_string(),
        "cancel" => err("no in-flight uploads to cancel — AF-5 wires the producer side"),
        other => err(format!("unknown outbox verb: {other}")),
    }
}

// ---- Downloads — action/files-downloads/<verb> --------------------

/// Action-topic prefix for the downloads surface.
pub const DOWNLOADS_PREFIX: &str = "files-downloads";
/// Verbs served on `action/files-downloads/<verb>`.
pub const DOWNLOADS_VERBS: [&str; 2] = ["list", "reveal"];

/// Reply builder for the downloads surface. `list` is honest empty
/// (local `~/Downloads` is served by mde-files's `LocalFsBackend`, not
/// this surface); `reveal` (body = id) has no mesh download recorded.
#[must_use]
pub fn downloads_reply(verb: &str, _body: Option<&str>) -> String {
    match verb {
        "list" => "[]".to_string(),
        "reveal" => err("no mesh downloads recorded — AF-5 wires the producer side"),
        other => err(format!("unknown downloads verb: {other}")),
    }
}

// ---- FileOperations — action/file-ops/<verb> ----------------------

/// Action-topic prefix for the file-operations surface.
pub const FILE_OPS_PREFIX: &str = "file-ops";
/// Verbs served on `action/file-ops/<verb>`.
pub const FILE_OPS_VERBS: [&str; 3] = ["send-to", "rollback", "audit-log"];

/// User-facing message when the operator tries a mesh send but no
/// transport is wired. A future epic dispatches `send-to` through the
/// `orchestrator` Send-To engine.
const SEND_TO_NOT_CONFIGURED: &str =
    "mesh send not configured — no transport (rsync / scp / qnm-share) is wired yet";

/// Reply builder for the file-operations surface. `send-to` (body =
/// `{sources,selector,mode,conflict}`) and `rollback` (body = op id)
/// honestly report no transport; `audit-log` is the honest empty log.
#[must_use]
pub fn file_ops_reply(verb: &str, _body: Option<&str>) -> String {
    match verb {
        "send-to" | "rollback" => err(SEND_TO_NOT_CONFIGURED),
        "audit-log" => "[]".to_string(),
        other => err(format!("unknown file-ops verb: {other}")),
    }
}

// ---- Fleet.Files — action/fleet-files/<verb> ----------------------

/// Action-topic prefix for the Fleet.Files surface.
pub const FLEET_FILES_PREFIX: &str = "fleet-files";
/// Verbs served on `action/fleet-files/<verb>`.
pub const FLEET_FILES_VERBS: [&str; 3] = ["peers", "self-node", "list-peer"];

/// The live mesh-roster surface. Reads from the mackesd SQLite store
/// (`nodes` table via `crate::store::list_nodes`) so mde-files's
/// mesh-browse gets a real peer roster. Holds the same `Arc`-shared
/// connection the reconcile worker upserts into, plus the host's own
/// identity.
#[derive(Debug, Clone)]
pub struct FleetFilesService {
    store: std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    host: String,
    node_id: String,
}

impl FleetFilesService {
    /// Build a service rooted at a live SQLite connection and the
    /// host's own identity.
    #[must_use]
    pub fn new(
        store: std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>,
        host: impl Into<String>,
        node_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            host: host.into(),
            node_id: node_id.into(),
        }
    }

    /// Sync reply builder for the Fleet.Files Bus verbs. Locks the
    /// shared store via `blocking_lock` — correct because the
    /// responder runs on a dedicated non-async thread (it would panic
    /// inside a tokio runtime).
    #[must_use]
    pub fn reply(&self, verb: &str, _body: Option<&str>) -> String {
        match verb {
            // JSON array of `WirePeer` rows from the live mesh roster,
            // excluding the local host (it surfaces via `self-node`).
            "peers" => {
                let nodes = {
                    let conn = self.store.blocking_lock();
                    match crate::store::list_nodes(&conn) {
                        Ok(n) => n,
                        Err(e) => return err(format!("list_nodes: {e}")),
                    }
                };
                let wires: Vec<WirePeer<'_>> = nodes
                    .iter()
                    .filter(|n| n.node_id != self.node_id)
                    .map(|n| WirePeer {
                        name: &n.name,
                        addr: n.region.as_deref().unwrap_or("—"),
                        kind: match n.role.as_str() {
                            "host" => "server",
                            "observer" => "ci",
                            _ => "desktop",
                        },
                        status: match n.health.as_str() {
                            "healthy" => "online",
                            "degraded" => "idle",
                            _ => "offline",
                        },
                    })
                    .collect();
                serde_json::to_string(&wires).unwrap_or_else(|e| err(format!("encode peers: {e}")))
            }
            // JSON-encoded `WireSelfNode` for the local host.
            "self-node" => {
                let wire = WireSelfNode {
                    host: &self.host,
                    role: "host",
                    region: "local",
                };
                serde_json::to_string(&wire)
                    .unwrap_or_else(|e| err(format!("encode self_node: {e}")))
            }
            // Per-peer file index isn't built yet (it lands with the
            // mesh file-sync subsystem); `[]` is the correct empty
            // state, not a stub — the client renders "no shared files".
            "list-peer" => "[]".to_string(),
            other => err(format!("unknown fleet-files verb: {other}")),
        }
    }
}

#[derive(serde::Serialize)]
struct WirePeer<'a> {
    name: &'a str,
    addr: &'a str,
    kind: &'a str,
    status: &'a str,
}

#[derive(serde::Serialize)]
struct WireSelfNode<'a> {
    host: &'a str,
    role: &'a str,
    region: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_prefixes_and_verbs_lock() {
        assert_eq!(INBOX_PREFIX, "files-inbox");
        assert_eq!(OUTBOX_PREFIX, "files-outbox");
        assert_eq!(DOWNLOADS_PREFIX, "files-downloads");
        assert_eq!(FILE_OPS_PREFIX, "file-ops");
        assert_eq!(FLEET_FILES_PREFIX, "fleet-files");
        assert_eq!(FLEET_FILES_VERBS, ["peers", "self-node", "list-peer"]);
    }

    // The four stub surfaces keep their honest empty /
    // transport-not-configured shape — a regression to a "Phase G"
    // jargon leak is caught here.
    #[test]
    fn inbox_list_is_honest_empty() {
        assert_eq!(inbox_reply("list", None), "[]");
    }

    #[test]
    fn outbox_list_is_honest_empty() {
        assert_eq!(outbox_reply("list", None), "[]");
    }

    #[test]
    fn downloads_list_is_honest_empty() {
        assert_eq!(downloads_reply("list", None), "[]");
    }

    #[test]
    fn file_ops_send_to_returns_transport_not_configured() {
        let msg = file_ops_reply("send-to", Some(r#"{"sources":[]}"#));
        assert!(
            msg.contains("transport") && msg.contains("not configured"),
            "expected human-readable 'transport not configured' message, got: {msg}"
        );
        // Negative: must not leak the old "Phase G" jargon.
        assert!(!msg.contains("Phase G"), "Phase G jargon leaked: {msg}");
    }

    #[test]
    fn file_ops_audit_log_is_honest_empty() {
        assert_eq!(file_ops_reply("audit-log", Some("100")), "[]");
    }

    #[test]
    fn unknown_verb_yields_error_envelope() {
        assert!(inbox_reply("bogus", None).contains("unknown inbox verb"));
        assert!(file_ops_reply("bogus", None).contains("unknown file-ops verb"));
    }

    #[test]
    fn fleet_files_peers_returns_empty_when_db_is_empty() {
        // In-memory connection with the nodes table migrated but no
        // rows → an empty roster, not an error.
        let conn = crate::store::open_in_memory().expect("open in-memory");
        let store = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
        let s = FleetFilesService::new(store, "test-host", "peer:test");
        assert_eq!(s.reply("peers", None), "[]");
    }

    #[test]
    fn fleet_files_self_node_encodes_hostname() {
        let conn = crate::store::open_in_memory().expect("open in-memory");
        let store = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
        let s = FleetFilesService::new(store, "anvil", "peer:anvil");
        let json = s.reply("self-node", None);
        assert!(json.contains("\"host\":\"anvil\""));
        assert!(json.contains("\"role\":"));
    }

    #[test]
    fn fleet_files_list_peer_returns_empty_array() {
        // The per-peer file index isn't built yet; `[]` is the correct
        // empty-state response.
        let conn = crate::store::open_in_memory().expect("open in-memory");
        let store = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
        let s = FleetFilesService::new(store, "test-host", "peer:test");
        assert_eq!(s.reply("list-peer", Some("birch")), "[]");
    }

    #[test]
    fn fleet_files_unknown_verb_yields_error_envelope() {
        let conn = crate::store::open_in_memory().expect("open in-memory");
        let store = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
        let s = FleetFilesService::new(store, "test-host", "peer:test");
        assert!(s.reply("bogus", None).contains("unknown fleet-files verb"));
    }
}
