//! PD-1 — `action/mesh/directory`: the joined per-peer record every
//! directory consumer reads (the Peers Front Door, `mackesd peers`,
//! Controller→Inventory — one verb, many faces; Q9/W7).
//!
//! The join sources only replicated / local truth:
//! - **PeerRecord** (`<root>/peers/*.json`, PEERVER) — hostname,
//!   mde_version, last_seen_ms, health.
//! - **Presence tier** (Q11) — computed from `last_seen_ms`:
//!   `online` ≤ 2 min, `idle` ≤ 10 min, else `offline`.
//! - **Revision currency** (Q15, FPG) — the elected head of the fleet
//!   log vs this peer's latest apply-ack: `synced` / `behind` /
//!   `unknown`.
//! - **Overlay IP + role** — from the local nebula roster mirror when
//!   available (`None` honestly when not).
//!
//! Voice presence + service descriptors join in PD-2 when peers
//! publish them; until then the fields are absent — no fake data (§7).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mackes_mesh_types::peers::{read_peers, PeerRecord};
use magic_fleet::store;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The directory verb's action topic.
pub const ACTION_TOPIC: &str = "action/mesh/directory";

/// PD-12 — the Wake-on-LAN verb: `{mac}` → broadcast the magic
/// packet from THIS box (the segment-sharing relay is a follow-up).
pub const WAKE_TOPIC: &str = "action/mesh/wake";

/// PD-11 — the lifecycle verb: `{peer, kind, name, op}` → a request
/// file on the replicated volume; the target's executor validates
/// against its own inventory (the L9 rail) + runs it.
pub const LIFECYCLE_TOPIC: &str = "action/services/lifecycle";

/// PD-11 — result retrieval: `{peer, id}` → the consumed result file
/// (`found: false` while the executor hasn't answered yet).
pub const LIFECYCLE_RESULT_TOPIC: &str = "action/services/lifecycle-result";

/// Responder poll interval (matches the fleet responder).
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Presence tier from heartbeat age (Q11).
#[must_use]
pub fn presence_tier(now_ms: u64, last_seen_ms: u64) -> &'static str {
    let age = now_ms.saturating_sub(last_seen_ms);
    if age <= 2 * 60 * 1000 {
        "online"
    } else if age <= 10 * 60 * 1000 {
        "idle"
    } else {
        "offline"
    }
}

/// Revision currency (Q15): this peer's latest applied ack vs the
/// elected head. `unknown` when the log is empty or the peer never
/// acked.
#[must_use]
pub fn revision_currency(head: Option<u64>, acked: Option<u64>) -> &'static str {
    match (head, acked) {
        (Some(h), Some(a)) if a >= h => "synced",
        (Some(_), Some(_)) => "behind",
        _ => "unknown",
    }
}

/// One peer's joined directory row.
#[must_use]
pub fn directory_row(
    rec: &PeerRecord,
    overlay: Option<&(String, String)>,
    head: Option<u64>,
    acked: Option<u64>,
    now_ms: u64,
) -> serde_json::Value {
    json!({
        "hostname": rec.hostname,
        "presence": presence_tier(now_ms, rec.last_seen_ms),
        "last_seen_ms": rec.last_seen_ms,
        "health": rec.health,
        "mde_version": rec.mde_version,
        "descriptors": rec.descriptors,
        "overlay_ip": overlay.map(|(ip, _)| ip.clone()),
        "role": overlay.map(|(_, role)| role.clone()),
        "revision": {
            "head": head,
            "acked": acked,
            "currency": revision_currency(head, acked),
        },
    })
}

/// Directory service — owns the source locations.
#[derive(Debug, Default, Clone)]
pub struct DirectoryService {
    /// The replicated workgroup root (PeerRecords + the fleet log).
    pub workgroup_root: PathBuf,
    /// Optional path to the mackesd SQLite store for the nebula
    /// roster mirror (overlay IP + role). `None` → those fields are
    /// `null` honestly.
    pub store_db: Option<PathBuf>,
}

impl DirectoryService {
    /// Service over the replicated root + optional roster DB.
    #[must_use]
    pub fn new(workgroup_root: &Path, store_db: Option<PathBuf>) -> Self {
        Self {
            workgroup_root: workgroup_root.to_path_buf(),
            store_db,
        }
    }

    /// Overlay-ip + role per hostname from the roster mirror.
    /// Empty on any failure (the join degrades, never errors).
    fn roster_index(&self) -> HashMap<String, (String, String)> {
        let Some(db) = &self.store_db else {
            return HashMap::new();
        };
        let Ok(conn) =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        else {
            return HashMap::new();
        };
        match crate::nebula_roster::export_roster(&conn) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| (r.name.clone(), (r.overlay_ip, r.groups)))
                .collect(),
            Err(_) => HashMap::new(),
        }
    }

    /// Build the full directory reply (the verb body + the CLI both
    /// call this).
    #[must_use]
    pub fn build_directory(&self, now_ms: u64) -> serde_json::Value {
        let records = read_peers(&mackes_mesh_types::peers::peers_dir(&self.workgroup_root));
        let roster = self.roster_index();
        let rev_dir = store::revisions_dir(&self.workgroup_root);
        let head = store::elect_head(&rev_dir).map(|r| r.version);
        // Latest applied ack per peer across all revisions.
        let mut acked: HashMap<String, u64> = HashMap::new();
        for r in store::read_revisions(&rev_dir) {
            for ack in store::read_acks(&self.workgroup_root, r.version) {
                if ack.status == "applied" {
                    let e = acked.entry(ack.peer).or_insert(0);
                    *e = (*e).max(r.version);
                }
            }
        }
        let rows: Vec<_> = records
            .iter()
            .map(|rec| {
                directory_row(
                    rec,
                    roster.get(&rec.hostname),
                    head,
                    acked.get(&rec.hostname).copied(),
                    now_ms,
                )
            })
            .collect();
        json!({ "ok": true, "head": head, "peers": rows })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Run the directory Bus responder until `should_stop()` — same
/// dedicated-OS-thread shape as the fleet responder.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DirectoryService, should_stop: F) {
    let mut cursor: Option<String> = None;
    let mut wake_cursor: Option<String> = None;
    let mut lifecycle_cursor: Option<String> = None;
    let mut result_cursor: Option<String> = None;
    while !should_stop() {
        poll_once(persist, svc, &mut cursor);
        poll_wake_once(persist, &mut wake_cursor);
        poll_lifecycle_once(persist, svc, &mut lifecycle_cursor);
        poll_lifecycle_result_once(persist, svc, &mut result_cursor);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// PD-11 — answer result polls.
pub fn poll_lifecycle_result_once(
    persist: &Persist,
    svc: &DirectoryService,
    cursor: &mut Option<String>,
) {
    let msgs = match persist.list_since(LIFECYCLE_RESULT_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(_) => return,
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let req: serde_json::Value = serde_json::from_str(msg.body.as_deref().unwrap_or("{}"))
            .unwrap_or(serde_json::Value::Null);
        let reply = match (
            req.get("peer").and_then(|v| v.as_str()),
            req.get("id").and_then(|v| v.as_str()),
        ) {
            (Some(peer), Some(id)) => {
                match crate::lifecycle::take_result(&svc.workgroup_root, peer, id) {
                    Some(r) => json!({ "ok": true, "found": true,
                                       "result": { "ok": r.ok, "error": r.error } }),
                    None => json!({ "ok": true, "found": false }),
                }
            }
            _ => json!({ "ok": false, "error": "lifecycle-result: need peer + id" }),
        }
        .to_string();
        let _ = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply),
        );
    }
}

/// PD-11 — accept lifecycle requests + write the replicated request
/// file. The reply carries the request id the GUI polls results by.
pub fn poll_lifecycle_once(persist: &Persist, svc: &DirectoryService, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(LIFECYCLE_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(_) => return,
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let reply = lifecycle_reply(svc, msg.body.as_deref(), &msg.ulid);
        let _ = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply),
        );
    }
}

/// Build the lifecycle-verb reply (the request file write is the
/// side effect; the message ulid doubles as the request id).
#[must_use]
pub fn lifecycle_reply(svc: &DirectoryService, body: Option<&str>, id: &str) -> String {
    let req: serde_json::Value =
        serde_json::from_str(body.unwrap_or("{}")).unwrap_or(serde_json::Value::Null);
    let get = |k: &str| req.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(peer), Some(kind), Some(name), Some(op)) =
        (get("peer"), get("kind"), get("name"), get("op"))
    else {
        return json!({ "ok": false, "error": "lifecycle: need peer/kind/name/op" }).to_string();
    };
    let request = crate::lifecycle::LifecycleRequest {
        id: id.to_string(),
        kind,
        name,
        op,
        from: "local-workbench".into(),
    };
    match crate::lifecycle::write_request(&svc.workgroup_root, &peer, &request) {
        Ok(_) => json!({ "ok": true, "id": id, "peer": peer }).to_string(),
        Err(e) => json!({ "ok": false, "error": format!("lifecycle: {e}") }).to_string(),
    }
}

/// PD-12 — answer wake requests: validate the MAC, fire the magic
/// packet (UDP/9 broadcast), reply `{ok}` / honest error.
pub fn poll_wake_once(persist: &Persist, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(WAKE_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(_) => return,
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let reply = wake_reply(msg.body.as_deref());
        let _ = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply),
        );
    }
}

/// Build the wake reply (pure-ish; the send is the side effect).
#[must_use]
pub fn wake_reply(body: Option<&str>) -> String {
    let req: serde_json::Value =
        serde_json::from_str(body.unwrap_or("{}")).unwrap_or(serde_json::Value::Null);
    let Some(mac_str) = req.get("mac").and_then(|v| v.as_str()) else {
        return json!({ "ok": false, "error": "wake: missing `mac`" }).to_string();
    };
    let Some(mac) = crate::workers::wol::normalize_mac(mac_str) else {
        return json!({ "ok": false, "error": format!("wake: bad mac {mac_str}") }).to_string();
    };
    match crate::workers::wol::wake(mac, "255.255.255.255", 9) {
        Ok(()) => json!({ "ok": true, "woke": mac_str }).to_string(),
        Err(e) => json!({ "ok": false, "error": format!("wake: {e}") }).to_string(),
    }
}

/// One poll sweep: each request on [`ACTION_TOPIC`] gets the full
/// joined directory on `reply/<ulid>`.
pub fn poll_once(persist: &Persist, svc: &DirectoryService, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "directory responder: list_since failed");
            return;
        }
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let reply = svc.build_directory(now_ms()).to_string();
        if let Err(e) = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply),
        ) {
            tracing::warn!(ulid = %msg.ulid, error = %e, "directory responder: reply write failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::peers::write_peer_record;

    #[test]
    fn presence_tiers_lock_q11() {
        let now = 100 * 60 * 1000;
        assert_eq!(presence_tier(now, now - 60 * 1000), "online");
        assert_eq!(presence_tier(now, now - 5 * 60 * 1000), "idle");
        assert_eq!(presence_tier(now, now - 11 * 60 * 1000), "offline");
    }

    #[test]
    fn currency_locks_q15() {
        assert_eq!(revision_currency(Some(3), Some(3)), "synced");
        assert_eq!(revision_currency(Some(3), Some(1)), "behind");
        assert_eq!(revision_currency(None, None), "unknown");
        assert_eq!(revision_currency(Some(3), None), "unknown");
    }

    #[test]
    fn directory_joins_records_with_fleet_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = now_ms();
        write_peer_record(
            &pdir,
            &PeerRecord {
                hostname: "pine".into(),
                mde_version: Some("4.2.1".into()),
                last_seen_ms: now,
                health: "healthy".into(),
                descriptors: None,
            },
        )
        .unwrap();
        // Fleet: head v2; pine acked v2.
        let rev_dir = store::revisions_dir(root);
        for v in [1u64, 2] {
            store::write_revision(
                &rev_dir,
                &magic_fleet::Revision {
                    version: v,
                    author: "peer:pine".into(),
                    at: v,
                    spec: magic_fleet::BaselineSpec::default(),
                },
            )
            .unwrap();
        }
        store::write_ack(
            root,
            2,
            &store::ApplyAck {
                peer: "pine".into(),
                status: "applied".into(),
                at: 5,
                detail: String::new(),
            },
        )
        .unwrap();

        let svc = DirectoryService::new(root, None);
        let dir = svc.build_directory(now);
        assert_eq!(dir["ok"], true);
        assert_eq!(dir["head"], 2);
        let p = &dir["peers"][0];
        assert_eq!(p["hostname"], "pine");
        assert_eq!(p["presence"], "online");
        assert_eq!(p["revision"]["currency"], "synced");
        assert_eq!(p["mde_version"], "4.2.1");
        // No roster DB → overlay/role are honest nulls.
        assert!(p["overlay_ip"].is_null());
    }

    #[test]
    fn wake_reply_validates_macs_honestly() {
        let bad: serde_json::Value = serde_json::from_str(&wake_reply(None)).unwrap();
        assert_eq!(bad["ok"], false);
        let garbage: serde_json::Value =
            serde_json::from_str(&wake_reply(Some(r#"{"mac":"zz:zz"}"#))).unwrap();
        assert_eq!(garbage["ok"], false);
        // A well-formed MAC broadcasts (UDP send to 255.255.255.255:9
        // succeeds without a listener — fire-and-forget by design).
        let ok: serde_json::Value =
            serde_json::from_str(&wake_reply(Some(r#"{"mac":"aa:bb:cc:dd:ee:ff"}"#))).unwrap();
        assert_eq!(ok["ok"], true);
    }

    #[test]
    fn lifecycle_verb_writes_the_request_file_and_refuses_bad_vocab() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DirectoryService::new(tmp.path(), None);
        let ok: serde_json::Value = serde_json::from_str(&lifecycle_reply(
            &svc,
            Some(r#"{"peer":"oak","kind":"container","name":"nginx","op":"start"}"#),
            "ulid-1",
        ))
        .unwrap();
        assert_eq!(ok["ok"], true);
        let pending = crate::lifecycle::take_requests(tmp.path(), "oak");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "ulid-1");
        let bad: serde_json::Value = serde_json::from_str(&lifecycle_reply(
            &svc,
            Some(r#"{"peer":"oak","kind":"container","name":"x","op":"explode"}"#),
            "ulid-2",
        ))
        .unwrap();
        assert_eq!(bad["ok"], false);
    }

    #[test]
    fn empty_mesh_is_an_ok_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DirectoryService::new(tmp.path(), None);
        let dir = svc.build_directory(now_ms());
        assert_eq!(dir["ok"], true);
        assert!(dir["peers"].as_array().unwrap().is_empty());
    }
}
