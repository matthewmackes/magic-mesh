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
///
/// `tags` are the peer's capability tags (L1 — `hop`/`execution`/
/// `headless`) read from the replicated tag manifest; empty when the
/// peer has none. The Peers Front Door renders them as chips and folds
/// them into the filter.
#[must_use]
pub fn directory_row(
    rec: &PeerRecord,
    overlay: Option<&(String, String)>,
    head: Option<u64>,
    acked: Option<u64>,
    tags: &[String],
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
        "tags": tags,
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
                // L1 — join the peer's capability tags from the
                // replicated manifest. Read-only; an absent manifest
                // is an honest empty tag list, never an error.
                let tags: Vec<String> =
                    mackes_mesh_types::cap_tags::read_tags(&self.workgroup_root, &rec.hostname)
                        .tags
                        .iter()
                        .map(|t| t.as_str().to_string())
                        .collect();
                directory_row(
                    rec,
                    roster.get(&rec.hostname),
                    head,
                    acked.get(&rec.hostname).copied(),
                    &tags,
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

/// PD-3 / Q10 — the topic the responder publishes a tiny "directory
/// changed" event on when the joined roster meaningfully changes, so the
/// Peers Front Door refreshes on **push** (not just its 30 s poll).
pub const EVENT_TOPIC: &str = "event/mesh/directory";

/// How many `POLL_INTERVAL` sweeps between change-detect passes (~3 s at
/// 400 ms) — frequent enough to feel live, rare enough not to re-read
/// every peer file 2½×/s.
const CHANGE_DETECT_EVERY: u64 = 8;

/// A digest of the *change-relevant* directory fields (hostnames,
/// presence, health, revision currency, tags).
///
/// A heartbeat-only `last_seen_ms` bump doesn't change it (no event
/// spam); a real presence/health/membership/tag change does.
#[must_use]
pub fn directory_digest(dir: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(peers) = dir.get("peers").and_then(|p| p.as_array()) {
        for p in peers {
            for k in ["hostname", "presence", "health"] {
                p.get(k).and_then(|v| v.as_str()).unwrap_or("").hash(&mut h);
            }
            p["revision"]["currency"]
                .as_str()
                .unwrap_or("")
                .hash(&mut h);
            for t in p
                .get("tags")
                .and_then(|t| t.as_array())
                .into_iter()
                .flatten()
            {
                t.as_str().unwrap_or("").hash(&mut h);
            }
        }
    }
    h.finish()
}

/// PD-3 / Q10 — publish a directory-changed event when the digest moves.
/// The first sweep only seeds `last` (startup state isn't a "change").
pub fn poll_directory_change(
    persist: &Persist,
    svc: &DirectoryService,
    last: &mut Option<u64>,
    now_ms: u64,
) {
    let digest = directory_digest(&svc.build_directory(now_ms));
    if *last == Some(digest) {
        return;
    }
    let seeding = last.is_none();
    *last = Some(digest);
    if !seeding {
        let body = json!({ "ok": true, "changed_at_ms": now_ms }).to_string();
        let _ = persist.write(EVENT_TOPIC, Priority::Default, None, Some(&body));
    }
}

/// Run the directory Bus responder until `should_stop()` — same
/// dedicated-OS-thread shape as the fleet responder.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DirectoryService, should_stop: F) {
    let mut cursor: Option<String> = None;
    let mut wake_cursor: Option<String> = None;
    let mut lifecycle_cursor: Option<String> = None;
    let mut result_cursor: Option<String> = None;
    let mut last_digest: Option<u64> = None;
    let mut tick: u64 = 0;
    while !should_stop() {
        poll_once(persist, svc, &mut cursor);
        poll_wake_once(persist, &mut wake_cursor);
        poll_lifecycle_once(persist, svc, &mut lifecycle_cursor);
        poll_lifecycle_result_once(persist, svc, &mut result_cursor);
        // PD-3/Q10 — push a change event ~every 3 s when the roster moves.
        if tick % CHANGE_DETECT_EVERY == 0 {
            poll_directory_change(persist, svc, &mut last_digest, now_ms());
        }
        tick = tick.wrapping_add(1);
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
        // EFF-23 — refuse an oversized body before from_str.
        if !crate::ipc::body_within_cap(msg.body.as_deref()) {
            tracing::warn!(
                topic = LIFECYCLE_RESULT_TOPIC,
                len = msg.body.as_ref().map_or(0, String::len),
                "directory responder: lifecycle-result body exceeds cap; refusing",
            );
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&crate::ipc::body_too_large_reply("lifecycle-result")),
            );
            continue;
        }
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
        // EFF-23 — refuse an oversized body before lifecycle_reply parses it.
        let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
            lifecycle_reply(svc, msg.body.as_deref(), &msg.ulid)
        } else {
            tracing::warn!(
                topic = LIFECYCLE_TOPIC,
                len = msg.body.as_ref().map_or(0, String::len),
                "directory responder: lifecycle body exceeds cap; refusing",
            );
            crate::ipc::body_too_large_reply("lifecycle")
        };
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
        // EFF-23 — refuse an oversized body before wake_reply parses it.
        let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
            wake_reply(msg.body.as_deref())
        } else {
            tracing::warn!(
                topic = WAKE_TOPIC,
                len = msg.body.as_ref().map_or(0, String::len),
                "directory responder: wake body exceeds cap; refusing",
            );
            crate::ipc::body_too_large_reply("wake")
        };
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
    fn directory_joins_capability_tags_l1() {
        use mackes_mesh_types::cap_tags::{write_tags, CapabilityTag, NodeTags};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = now_ms();
        write_peer_record(
            &pdir,
            &PeerRecord {
                hostname: "anvil".into(),
                mde_version: Some("4.2.1".into()),
                last_seen_ms: now,
                health: "healthy".into(),
                descriptors: None,
            },
        )
        .unwrap();
        let mut t = NodeTags::default();
        t.tags.insert(CapabilityTag::Execution);
        t.tags.insert(CapabilityTag::Headless);
        write_tags(root, "anvil", &t).unwrap();

        let svc = DirectoryService::new(root, None);
        let dir = svc.build_directory(now);
        let tags = dir["peers"][0]["tags"].as_array().unwrap();
        let tags: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
        assert!(tags.contains(&"execution"));
        assert!(tags.contains(&"headless"));
        assert!(!tags.contains(&"hop"));
    }

    #[test]
    fn directory_tags_are_empty_without_a_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = now_ms();
        write_peer_record(
            &pdir,
            &PeerRecord {
                hostname: "bare".into(),
                mde_version: None,
                last_seen_ms: now,
                health: "unknown".into(),
                descriptors: None,
            },
        )
        .unwrap();
        let svc = DirectoryService::new(root, None);
        let dir = svc.build_directory(now);
        assert!(dir["peers"][0]["tags"].as_array().unwrap().is_empty());
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
    fn directory_digest_ignores_heartbeat_bumps_but_catches_real_changes() {
        // PD-3/Q10 — last_seen_ms bumps must not change the digest (no
        // event spam); presence/health/tags must.
        let base = json!({"peers":[{
            "hostname":"pine","presence":"online","health":"healthy",
            "last_seen_ms": 100, "revision":{"currency":"synced"}, "tags":["execution"],
        }]});
        let mut bumped = base.clone();
        bumped["peers"][0]["last_seen_ms"] = json!(999_999);
        assert_eq!(directory_digest(&base), directory_digest(&bumped));

        let mut degraded = base.clone();
        degraded["peers"][0]["health"] = json!("degraded");
        assert_ne!(directory_digest(&base), directory_digest(&degraded));

        let mut retagged = base.clone();
        retagged["peers"][0]["tags"] = json!(["execution", "headless"]);
        assert_ne!(directory_digest(&base), directory_digest(&retagged));
    }

    #[test]
    fn poll_directory_change_seeds_then_publishes_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = now_ms();
        let mut rec = PeerRecord {
            hostname: "pine".into(),
            mde_version: Some("4.2.1".into()),
            last_seen_ms: now,
            health: "healthy".into(),
            descriptors: None,
        };
        write_peer_record(&pdir, &rec).unwrap();

        let bus = tmp.path().join("bus");
        let persist = mde_bus::persist::Persist::open(bus).unwrap();
        let svc = DirectoryService::new(root, None);
        let mut last: Option<u64> = None;

        // First sweep seeds — no event.
        poll_directory_change(&persist, &svc, &mut last, now);
        assert!(persist.list_since(EVENT_TOPIC, None).unwrap().is_empty());
        // No change → still no event.
        poll_directory_change(&persist, &svc, &mut last, now);
        assert!(persist.list_since(EVENT_TOPIC, None).unwrap().is_empty());
        // A real change (health) → one event.
        rec.health = "degraded".into();
        write_peer_record(&pdir, &rec).unwrap();
        poll_directory_change(&persist, &svc, &mut last, now);
        assert_eq!(persist.list_since(EVENT_TOPIC, None).unwrap().len(), 1);
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
