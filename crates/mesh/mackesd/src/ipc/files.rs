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

/// Reply builder for the file-operations surface when no transport is
/// available (no QNM-Shared root) — kept for the degraded path.
#[must_use]
pub fn file_ops_reply(verb: &str, _body: Option<&str>) -> String {
    match verb {
        "send-to" | "rollback" => err(SEND_TO_NOT_CONFIGURED),
        "audit-log" => "[]".to_string(),
        other => err(format!("unknown file-ops verb: {other}")),
    }
}

// ---- FileXfer — the real cross-mesh file transport (AUD-1 / AUD-7) ----
//
// Send-To moves bytes over the **LizardFS-replicated QNM-Shared volume** —
// no extra transport daemon, the replicated volume IS the wire. Sending to
// peer `P` copies each source into `<qnm>/inbox/<P>/<self>/<name>`; LizardFS
// replication delivers it to P, whose Inbox view lists `<qnm>/inbox/<P>/**`
// (the sender is the subdirectory name — attribution for free). The sender's
// own record is appended to `<qnm>/outbox/<self>.jsonl`. (Closes the §7
// "send not configured" stub + the AF-5 empty-inbox producer.)

/// The replicated-volume file transport backing the file-ops / inbox /
/// outbox Bus surfaces. Holds the QNM-Shared root + this node's host id.
pub struct FileXfer {
    qnm_root: std::path::PathBuf,
    host: String,
}

impl FileXfer {
    /// Construct over the QNM-Shared root + this node's host identity.
    #[must_use]
    pub fn new(qnm_root: std::path::PathBuf, host: String) -> Self {
        Self { qnm_root, host }
    }

    fn inbox_root(&self, peer: &str) -> std::path::PathBuf {
        self.qnm_root.join("inbox").join(peer)
    }

    fn outbox_log(&self) -> std::path::PathBuf {
        self.qnm_root.join("outbox").join(format!("{}.jsonl", self.host))
    }

    /// `action/file-ops/<verb>` — `send-to` / `rollback` / `audit-log`.
    #[must_use]
    pub fn file_ops_reply(&self, verb: &str, body: Option<&str>) -> String {
        match verb {
            "send-to" => self.send_to(body),
            "rollback" => self.rollback(body),
            "audit-log" => self.read_outbox_rows(),
            other => err(format!("unknown file-ops verb: {other}")),
        }
    }

    /// `action/files-inbox/<verb>` — `list` reads this node's replicated inbox.
    #[must_use]
    pub fn inbox_reply(&self, verb: &str, _body: Option<&str>) -> String {
        match verb {
            "list" => self.inbox_list(),
            "mark-opened" => json!({ "ok": true }).to_string(),
            other => err(format!("unknown inbox verb: {other}")),
        }
    }

    /// `action/files-outbox/<verb>` — `list` reads this node's send record.
    #[must_use]
    pub fn outbox_reply(&self, verb: &str, _body: Option<&str>) -> String {
        match verb {
            "list" => self.read_outbox_rows(),
            "cancel" => json!({ "ok": true }).to_string(),
            other => err(format!("unknown outbox verb: {other}")),
        }
    }

    /// Copy each source into the target peer's replicated inbox.
    fn send_to(&self, body: Option<&str>) -> String {
        let Some(v) = body.and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok()) else {
            return err("send-to: missing/invalid body (need {sources,selector,mode,conflict})");
        };
        // Selector grammar: `peer:<name>` is the direct-delivery target. group/
        // role/site fan-out is a follow-up — report honestly rather than guess.
        let selector = v
            .get("selector")
            .or_else(|| v.get("destination"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let Some(target) = selector.strip_prefix("peer:").filter(|t| !t.is_empty()) else {
            return err(format!(
                "send-to: only peer: destinations deliver directly (got '{selector}')"
            ));
        };
        let sources: Vec<String> = v
            .get("sources")
            .and_then(|s| s.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if sources.is_empty() {
            return err("send-to: no sources");
        }
        let conflict = v.get("conflict").and_then(serde_json::Value::as_str).unwrap_or("rename");
        let mode = v.get("mode").and_then(serde_json::Value::as_str).unwrap_or("copy");
        let dest_dir = self.inbox_root(target).join(&self.host);
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            return err(format!("send-to: cannot open inbox {}: {e}", dest_dir.display()));
        }
        let mut delivered: Vec<String> = Vec::new();
        let mut bytes: u64 = 0;
        for src in &sources {
            let src_path = std::path::Path::new(src);
            let Some(name) = src_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(dest) = resolve_conflict(&dest_dir, name, conflict) else {
                continue; // skip policy, or unresolved
            };
            match std::fs::copy(src_path, &dest) {
                Ok(n) => {
                    bytes += n;
                    delivered.push(name.to_string());
                    if mode == "move" {
                        let _ = std::fs::remove_file(src_path);
                    }
                }
                Err(e) => return err(format!("send-to: copy {name} failed: {e}")),
            }
        }
        let op_id = now_ms();
        self.append_outbox(op_id, target, mode, bytes, &delivered);
        json!({
            "ok": true,
            "op_id": op_id,
            "count": delivered.len(),
            "bytes": bytes,
        })
        .to_string()
    }

    /// Undo a prior send: remove the files it delivered (looked up in the
    /// outbox log by op id) from the target's inbox.
    fn rollback(&self, body: Option<&str>) -> String {
        let op_id = body
            .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
            .and_then(|v| {
                v.get("op_id")
                    .or_else(|| v.get("id"))
                    .and_then(serde_json::Value::as_i64)
            });
        let Some(op_id) = op_id else {
            return err("rollback: need {op_id}");
        };
        let mut removed = 0;
        for row in self.outbox_rows() {
            if row.get("op_id").and_then(serde_json::Value::as_i64) != Some(op_id) {
                continue;
            }
            let target = row.get("target").and_then(serde_json::Value::as_str).unwrap_or_default();
            if let Some(files) = row.get("files").and_then(|f| f.as_array()) {
                for f in files.iter().filter_map(|x| x.as_str()) {
                    let p = self.inbox_root(target).join(&self.host).join(f);
                    if std::fs::remove_file(&p).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        json!({ "ok": true, "removed": removed }).to_string()
    }

    /// List this node's replicated inbox: every file under
    /// `<qnm>/inbox/<self>/<sender>/` as a `WireFileRow` (the sender is the
    /// subdir name → the `peer`/"from" column).
    fn inbox_list(&self) -> String {
        let root = self.inbox_root(&self.host);
        let mut rows: Vec<serde_json::Value> = Vec::new();
        let Ok(senders) = std::fs::read_dir(&root) else {
            return "[]".to_string();
        };
        for sender_entry in senders.flatten() {
            let sender = sender_entry.file_name().to_string_lossy().into_owned();
            let Ok(files) = std::fs::read_dir(sender_entry.path()) else {
                continue;
            };
            for f in files.flatten() {
                let meta = match f.metadata() {
                    Ok(m) if m.is_file() => m,
                    _ => continue,
                };
                let name = f.file_name().to_string_lossy().into_owned();
                rows.push(json!({
                    "name": name,
                    "size": meta.len(),
                    "mime": guess_mime(&f.file_name().to_string_lossy()),
                    "peer": sender,
                    "modified_ms": mtime_ms(&meta),
                }));
            }
        }
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    fn append_outbox(&self, op_id: i64, target: &str, mode: &str, bytes: u64, files: &[String]) {
        let log = self.outbox_log();
        if let Some(parent) = log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let row = json!({
            "op_id": op_id,
            "at_ms": op_id,
            "target": target,
            "mode": mode,
            "count": files.len(),
            "bytes": bytes,
            "files": files,
        });
        use std::io::Write;
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(fh, "{row}");
        }
    }

    fn outbox_rows(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.outbox_log())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .collect()
    }

    /// The outbox/audit log as a JSON array (newest first).
    fn read_outbox_rows(&self) -> String {
        let mut rows = self.outbox_rows();
        rows.reverse();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Resolve the destination path for `name` in `dir` under a conflict policy.
/// `None` = skip (existing + skip policy, or no resolvable name).
fn resolve_conflict(dir: &std::path::Path, name: &str, conflict: &str) -> Option<std::path::PathBuf> {
    let dest = dir.join(name);
    if !dest.exists() {
        return Some(dest);
    }
    match conflict {
        "overwrite" => Some(dest),
        "skip" => None,
        // "rename" / "ask" (daemon has no prompt) → find a free " (n)" name.
        _ => {
            let p = std::path::Path::new(name);
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
            let ext = p.extension().and_then(|e| e.to_str());
            for n in 1..1000 {
                let candidate = match ext {
                    Some(e) => dir.join(format!("{stem} ({n}).{e}")),
                    None => dir.join(format!("{stem} ({n})")),
                };
                if !candidate.exists() {
                    return Some(candidate);
                }
            }
            None
        }
    }
}

/// Wall-clock ms (the op id + delivery timestamp).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// File mtime as epoch-ms (0 on error).
fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Coarse mime tag from a filename extension (matches `WireFileRow.mime`).
fn guess_mime(name: &str) -> &'static str {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => "image",
        "pdf" => "pdf",
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" => "archive",
        "iso" | "qcow2" | "img" | "raw" => "disk",
        _ => "doc",
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

    // ---- FileXfer: the real QNM-Shared transport (AUD-1/AUD-7) --------

    #[test]
    fn send_to_delivers_into_target_inbox_and_recipient_lists_it() {
        let tmp = tempfile::tempdir().unwrap();
        let qnm = tmp.path().to_path_buf();
        // A source file on the sender ("pine").
        let src = tmp.path().join("notes.md");
        std::fs::write(&src, b"hello mesh").unwrap();

        let pine = FileXfer::new(qnm.clone(), "pine".to_string());
        let body = json!({
            "sources": [src.to_string_lossy()],
            "selector": "peer:oak",
            "mode": "copy",
            "conflict": "rename",
        })
        .to_string();
        let reply: serde_json::Value =
            serde_json::from_str(&pine.file_ops_reply("send-to", Some(&body))).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["count"], 1);
        assert_eq!(reply["bytes"], 10);

        // The file landed under inbox/oak/pine/ (sender = subdir).
        assert!(qnm.join("inbox/oak/pine/notes.md").exists());

        // oak lists its inbox and sees the file attributed to pine.
        let oak = FileXfer::new(qnm.clone(), "oak".to_string());
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&oak.inbox_reply("list", None)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "notes.md");
        assert_eq!(rows[0]["peer"], "pine");
        assert_eq!(rows[0]["size"], 10);
    }

    #[test]
    fn send_to_move_removes_source_and_rollback_undoes_delivery() {
        let tmp = tempfile::tempdir().unwrap();
        let qnm = tmp.path().to_path_buf();
        let src = tmp.path().join("doc.txt");
        std::fs::write(&src, b"abc").unwrap();
        let pine = FileXfer::new(qnm.clone(), "pine".to_string());
        let body = json!({"sources":[src.to_string_lossy()],"selector":"peer:oak","mode":"move"})
            .to_string();
        let reply: serde_json::Value =
            serde_json::from_str(&pine.file_ops_reply("send-to", Some(&body))).unwrap();
        let op_id = reply["op_id"].as_i64().unwrap();
        assert!(!src.exists(), "move removes the source");
        assert!(qnm.join("inbox/oak/pine/doc.txt").exists());
        // Rollback removes the delivered copy.
        let rb: serde_json::Value = serde_json::from_str(
            &pine.file_ops_reply("rollback", Some(&json!({ "op_id": op_id }).to_string())),
        )
        .unwrap();
        assert_eq!(rb["removed"], 1);
        assert!(!qnm.join("inbox/oak/pine/doc.txt").exists());
    }

    #[test]
    fn send_to_rejects_non_peer_selector_and_empty_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let x = FileXfer::new(tmp.path().to_path_buf(), "pine".to_string());
        assert!(x
            .file_ops_reply("send-to", Some(r#"{"sources":["/x"],"selector":"group:crew"}"#))
            .contains("peer:"));
        assert!(x
            .file_ops_reply("send-to", Some(r#"{"sources":[],"selector":"peer:oak"}"#))
            .contains("no sources"));
    }

    #[test]
    fn rename_conflict_keeps_both_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let qnm = tmp.path().to_path_buf();
        let src = tmp.path().join("a.txt");
        std::fs::write(&src, b"v1").unwrap();
        let pine = FileXfer::new(qnm.clone(), "pine".to_string());
        let body =
            json!({"sources":[src.to_string_lossy()],"selector":"peer:oak","conflict":"rename"})
                .to_string();
        let _ = pine.file_ops_reply("send-to", Some(&body));
        let _ = pine.file_ops_reply("send-to", Some(&body));
        let dir = qnm.join("inbox/oak/pine");
        assert!(dir.join("a.txt").exists());
        assert!(dir.join("a (1).txt").exists(), "second copy renamed, not clobbered");
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
