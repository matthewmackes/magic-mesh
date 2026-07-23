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

/// Retired PD-11 direct lifecycle topic. It is still drained so old clients get
/// an explicit refusal, but it never writes a request. Mutations must use the
/// authenticated, typed, audited `action/exec/request` worker.
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
///
/// `lighthouse_ips` is the canonical mesh-wide lighthouse overlay-IP set
/// (the replicated Nebula bundle's `static_host_map` roster — the SAME
/// fact `nebula_supervisor` uses: "a node is a lighthouse iff its overlay
/// IP is in the bundle's lighthouse set"). It's the role source of last
/// resort, consulted when none of the per-peer signals carry a role — see
/// the role-precedence note below.
#[must_use]
pub fn directory_row(
    rec: &PeerRecord,
    overlay: Option<&(String, String)>,
    head: Option<u64>,
    acked: Option<u64>,
    tags: &[String],
    lighthouse_ips: &[String],
    now_ms: u64,
) -> serde_json::Value {
    // The effective overlay IP this row reports (the peer's own record wins,
    // then the signer roster mirror) — also what we match against the
    // canonical lighthouse roster below.
    let overlay_ip = rec
        .overlay_ip
        .clone()
        .or_else(|| overlay.map(|(ip, _)| ip.clone()));
    // Resolve the effective role ONCE (precedence note below) so both the
    // `role` field and the MEDIA-1 class derivation read the same value.
    let row_role = overlay
        .map(|(_, role)| role.clone())
        .or_else(|| rec.role.clone().filter(|r| !r.trim().is_empty()))
        .unwrap_or_else(|| {
            if tags.iter().any(|t| t.eq_ignore_ascii_case("lighthouse"))
                || overlay_ip
                    .as_deref()
                    .is_some_and(|ip| lighthouse_ips.iter().any(|lh| lh == ip))
            {
                "lighthouse".to_string()
            } else {
                "peer".to_string()
            }
        });
    // Thin-lighthouse policy: legacy media markers are never surfaced as a
    // supported class or capability. Keep the JSON keys for old clients, but
    // force the values to the plain role so stale replicated rows cannot revive
    // media discovery or service placement.
    let is_media_row = false;
    json!({
        "hostname": rec.hostname,
        "presence": presence_tier(now_ms, rec.last_seen_ms),
        "last_seen_ms": rec.last_seen_ms,
        "health": rec.health,
        "mde_version": rec.mde_version,
        "descriptors": rec.descriptors,
        // Prefer the overlay IP the peer recorded into its own replicated
        // record (available mesh-wide); fall back to the local nebula
        // roster mirror (only populated on the signer). This is what lets
        // a peer's Mesh DNS / Service Publishing / Routing panels resolve
        // overlay addresses instead of showing empty.
        "overlay_ip": overlay_ip,
        // Role precedence: the signer's roster mirror (authoritative, but
        // only populated on the signer) → the peer's OWN replicated
        // `rec.role` (self-declared, available mesh-wide under SUBSTRATE-V2)
        // → capability-tag derivation → the canonical lighthouse roster
        // (the replicated bundle's overlay-IP set) → "peer".
        //
        // Trusting `rec.role` fixed HA-4: without it every peer collapsed
        // to "peer". But `rec.role` is itself empty on the pre-role peer
        // writers still live on the fleet (found on Eagle 11.0.5: the
        // directory carried role="-" for both lighthouses while `healthz`
        // counted 2), so the row must ALSO fall back to the canonical
        // lighthouse roster — a node whose overlay IP is in the replicated
        // bundle's `static_host_map` set IS a lighthouse, mesh-wide,
        // regardless of what its own record happens to declare. This is the
        // SAME detection `mesh_health_counts` feeds `lighthouse_count`, so
        // `action/mesh/directory` and `healthz` can no longer diverge.
        "role": row_role.clone(),
        "tags": tags,
        // Retired media compatibility fields. Current lighthouses are always
        // plain control-plane nodes.
        "media": is_media_row,
        "class": if is_media_row {
            mackes_mesh_types::lighthouse::LIGHTHOUSE_MEDIA_CLASS.to_string()
        } else {
            row_role.clone()
        },
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

    /// The canonical mesh-wide lighthouse overlay-IP set — the Nebula bundle's
    /// `static_host_map` roster, the SAME fact `nebula_supervisor` keys a node's
    /// lighthouse role off ("a node is a lighthouse iff its overlay IP is in the
    /// bundle's lighthouse set"). The bundle is replicated per-peer under
    /// `<root>/<node_id>/mackesd/nebula-bundle.json`.
    ///
    /// We UNION every readable bundle's lighthouse IPs rather than trusting any
    /// single one: the per-peer bundles are NOT all written with the full roster.
    /// The `/enroll` listener writes the complete redundant set (LIGHTHOUSE-10),
    /// but the auto-signer (`nebula_csr_watcher`) and `mesh_init` write a
    /// single-self entry — so reading one arbitrary bundle (whichever `read_dir`
    /// happened to surface first) would intermittently under-count and re-open the
    /// very HA-4 divergence this fixes. The union is order-independent (sorted +
    /// deduped), so every node computes the same set from the same replicated
    /// state. Empty when no bundle is reachable (the join degrades to the per-peer
    /// role signals, never errors) — the pre-bundle / unjoined state, where
    /// there's no lighthouse to surface anyway.
    ///
    /// This is what lets `directory_row` resolve `role=lighthouse` mesh-wide even
    /// when a peer's own `rec.role` is empty (the pre-role writers still live on
    /// the fleet), keeping `action/mesh/directory` and the `healthz`
    /// `lighthouse_count` (both built off `directory_row`) from diverging.
    #[must_use]
    pub fn lighthouse_overlay_ips(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.workgroup_root) else {
            return out;
        };
        for entry in entries.flatten() {
            // Per-peer dirs hold the replicated bundle at
            // `<node_id>/mackesd/nebula-bundle.json`. Union them all — a skinny
            // (self-only) bundle must not shadow a peer's full roster.
            let bundle_path = entry
                .path()
                .join("mackesd")
                .join(crate::ca::bundle::BUNDLE_FILENAME);
            if let Ok(bundle) = crate::ca::bundle::read_bundle(&bundle_path) {
                for lh in &bundle.lighthouses {
                    if !lh.overlay_ip.is_empty() {
                        out.push(lh.overlay_ip.clone());
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// ONBOARD-6 (OB6-FIX-4) — `(node_count, healthy, degraded, unreachable,
    /// is_leader, lighthouse_count)` for the healthz surfaces, from the LIVE
    /// directory + the leader lease rather than the store's enrolled-nodes table.
    /// This is the same peer set `mackesd peers` shows, so the Mesh-Control
    /// healthz card now matches the Inventory + reflects the elected leader. The
    /// trailing `lighthouse_count` drives HA-4's `degraded: no HA` posture.
    #[must_use]
    pub fn mesh_health_counts(
        &self,
        node_id: &str,
        now_ms: u64,
    ) -> (u32, u32, u32, u32, bool, u32) {
        let dir = self.build_directory(now_ms);
        let peers = dir["peers"].as_array().cloned().unwrap_or_default();
        let total = u32::try_from(peers.len()).unwrap_or(u32::MAX);
        let (mut healthy, mut degraded, mut unreachable) = (0u32, 0u32, 0u32);
        // HA-4 — count the lighthouses in the live directory so healthz can flag
        // `degraded: no HA` below the 2-lighthouse floor. The directory's `role`
        // is "lighthouse" for tagged peers (see `build_directory`).
        let mut lighthouses = 0u32;
        for p in &peers {
            match p["health"].as_str() {
                Some("healthy") => healthy += 1,
                Some("degraded") => degraded += 1,
                _ => {}
            }
            if p["presence"].as_str() == Some("offline") {
                unreachable += 1;
            }
            if p["role"].as_str() == Some("lighthouse") {
                lighthouses += 1;
            }
        }
        // SUBSTRATE-4/8 — leadership from the shared leader read (etcd lease when
        // on the coordination plane, else the fs lockfile). `node_id` may carry
        // the `peer:` prefix; `leader_name` returns the bare hostname.
        let bare = node_id.strip_prefix("peer:").unwrap_or(node_id);
        let is_leader = self.leader_name().is_some_and(|l| l == bare);
        (
            total,
            healthy,
            degraded,
            unreachable,
            is_leader,
            lighthouses,
        )
    }

    /// Build the full directory reply (the verb body + the CLI both
    /// call this).
    #[must_use]
    pub fn build_directory(&self, now_ms: u64) -> serde_json::Value {
        // SUBSTRATE-3 — read the peer directory from etcd when this node is on
        // the coordination plane (endpoints file present), falling back to the
        // replicated fs union on any etcd error. Empty endpoints (every
        // pre-cutover node) ⇒ the fs path, unchanged. This responder runs on a
        // dedicated thread (off the tokio executor), so the blocking etcd read
        // is safe.
        let etcd_endpoints = crate::substrate::etcd::default_endpoints();
        let records = if etcd_endpoints.is_empty() {
            read_peers(&mackes_mesh_types::peers::peers_dir(&self.workgroup_root))
        } else {
            crate::substrate::peers::read_peers_blocking(&etcd_endpoints).unwrap_or_else(|| {
                read_peers(&mackes_mesh_types::peers::peers_dir(&self.workgroup_root))
            })
        };
        let roster = self.roster_index();
        // SUBSTRATE-8 / HA-4 — the canonical mesh-wide lighthouse roster
        // (bundle `static_host_map` overlay IPs), the role source of last
        // resort `directory_row` consults when no per-peer signal carries one.
        let lighthouse_ips = self.lighthouse_overlay_ips();
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
                    &lighthouse_ips,
                    now_ms,
                )
            })
            .collect();
        json!({
            "ok": true,
            "head": head,
            "leader": self.leader_name(),
            "leader_lease": self.leader_lease(),
            "peers": rows,
        })
    }

    /// SUBSTRATE-8 — the raw encoded leader lease (`node_id\trenewed_at_s\tepoch`)
    /// for surfaces that show the epoch/age (Mesh Control), or `None` when there's
    /// no live leader. etcd lease (re-encoded) when on the coordination plane,
    /// else the fs lockfile body. Off-tokio responder thread ⇒ blocking read safe.
    #[must_use]
    pub fn leader_lease(&self) -> Option<String> {
        let etcd_endpoints = crate::substrate::etcd::default_endpoints();
        if etcd_endpoints.is_empty() {
            std::fs::read_to_string(self.workgroup_root.join(".mackesd-leader.lock"))
                .ok()
                .filter(|s| !s.trim().is_empty())
        } else {
            crate::substrate::leader::current_leader_blocking(&etcd_endpoints).map(|l| l.encode())
        }
    }

    /// SUBSTRATE-4/8 — the current mesh leader's bare hostname (the leader-lease
    /// holder), or `None` when there's no live leader. Reads the etcd leader when
    /// on the coordination plane (endpoints present), else the fs lockfile lease.
    /// Runs on the off-tokio directory responder thread, so the blocking etcd
    /// read is safe. The `peer:` node-id prefix is stripped to match hostnames.
    #[must_use]
    pub fn leader_name(&self) -> Option<String> {
        let etcd_endpoints = crate::substrate::etcd::default_endpoints();
        let node_id = if etcd_endpoints.is_empty() {
            crate::leader::read_current_lease(&self.workgroup_root.join(".mackesd-leader.lock"))
                .map(|l| l.node_id)
        } else {
            crate::substrate::leader::current_leader_blocking(&etcd_endpoints).map(|l| l.node_id)
        }?;
        Some(
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
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

/// Drain the retired direct lifecycle topic and reply with an explicit refusal.
/// Keeping the responder during cutover prevents old clients from mistaking a
/// timeout for success while closing the public-Bus mutation bypass.
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

/// Refuse the retired unauthenticated lifecycle verb. The typed action worker is
/// the sole writer of replicated lifecycle requests.
#[must_use]
pub fn lifecycle_reply(_svc: &DirectoryService, _body: Option<&str>, _id: &str) -> String {
    json!({
        "ok": false,
        "error": "direct service lifecycle is retired; use authenticated action/exec/request"
    })
    .to_string()
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
                overlay_ip: None,
                role: None,
                external_addr: None,
                media: false,
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
    fn directory_row_propagates_the_peers_own_role_so_ha_counts_lighthouses() {
        // Regression (found live 2026-06-23 on a 2-lighthouse mesh): a peer's
        // replicated record carries role="lighthouse", but the directory row
        // collapsed every peer to "peer" — it consulted only the signer-only
        // roster mirror + capability tags, never `rec.role` — so
        // `mesh_health_counts` reported 0 lighthouses and `ha_ok` never flipped.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = now_ms();
        for h in ["lh-a", "lh-b"] {
            write_peer_record(
                &pdir,
                &PeerRecord {
                    hostname: h.into(),
                    mde_version: Some("11.0.2".into()),
                    last_seen_ms: now,
                    health: "healthy".into(),
                    descriptors: None,
                    overlay_ip: Some("10.42.0.9".into()),
                    role: Some("lighthouse".into()),
                    external_addr: None,
                    media: false,
                },
            )
            .unwrap();
        }
        let svc = DirectoryService::new(root, None); // no roster DB (non-signer)
        let dir = svc.build_directory(now);
        // Each row reflects the peer's OWN declared role, not a flat "peer".
        for p in dir["peers"].as_array().unwrap() {
            assert_eq!(p["role"], "lighthouse", "rec.role must propagate mesh-wide");
        }
        // …and the HA counter sees both, so ha_ok flips at the 2-LH floor.
        let (_total, _healthy, _deg, _unreach, _leader, lighthouses) =
            svc.mesh_health_counts("lh-a", now);
        assert_eq!(lighthouses, 2, "both live lighthouses are counted");
        assert!(
            !mackes_mesh_types::lighthouse::ha_degraded(lighthouses as usize),
            "2 lighthouses meet the HA floor → not degraded"
        );
    }

    #[test]
    fn directory_row_drops_retired_media_lighthouse_markers() {
        let now = now_ms();
        let media_lh = PeerRecord {
            hostname: "media-lh".into(),
            mde_version: None,
            last_seen_ms: now,
            health: "healthy".into(),
            descriptors: None,
            overlay_ip: Some("10.42.0.9".into()),
            role: Some("lighthouse".into()),
            external_addr: None,
            media: true,
        };
        let row = directory_row(&media_lh, None, None, None, &[], &[], now);
        assert_eq!(row["role"], "lighthouse", "still a lighthouse role");
        assert_eq!(row["media"], false);
        assert_eq!(row["class"], "lighthouse");

        // A plain lighthouse: media=false, class=role.
        let mut plain = media_lh.clone();
        plain.media = false;
        let plain_row = directory_row(&plain, None, None, None, &[], &[], now);
        assert_eq!(plain_row["media"], false);
        assert_eq!(plain_row["class"], "lighthouse");

        // A server that mis-claims the media tag is NOT the media subclass
        // (the tag is meaningless without the lighthouse role).
        let mut srv = media_lh.clone();
        srv.role = Some("server".into());
        srv.media = true;
        let srv_row = directory_row(&srv, None, None, None, &[], &[], now);
        assert_eq!(srv_row["media"], false, "media tag dropped off a server");
        assert_eq!(srv_row["class"], "server");
    }

    #[test]
    fn directory_row_resolves_lighthouse_from_the_canonical_roster_when_rec_role_empty() {
        // Regression (found live on Eagle 11.0.5): the directory carried role
        // "-" (empty `rec.role`) for both online lighthouses — they were written
        // by a pre-role peer writer — while `healthz` reported lighthouse_count:2.
        // `action/mesh/directory` (the Workbench Lighthouses panel's source) and
        // the healthz path BOTH build off `directory_row`, so the row must fall
        // back to the canonical mesh-wide lighthouse detection: a node whose
        // overlay IP is in the replicated Nebula bundle's `static_host_map`
        // roster IS a lighthouse, regardless of what its own record declares.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pdir = mackes_mesh_types::peers::peers_dir(root);
        std::fs::create_dir_all(&pdir).unwrap();
        let now = now_ms();

        // Two lighthouses with EMPTY rec.role (the live-Eagle shape) + a plain
        // peer that is NOT in the lighthouse roster.
        let lhs = [("lh-1", "10.42.0.1"), ("lh-2", "10.42.0.3")];
        for (h, ip) in lhs {
            write_peer_record(
                &pdir,
                &PeerRecord {
                    hostname: h.into(),
                    mde_version: Some("11.0.5".into()),
                    last_seen_ms: now,
                    health: "healthy".into(),
                    descriptors: None,
                    overlay_ip: Some(ip.into()),
                    role: None, // pre-role writer — the bug condition
                    external_addr: None,
                    media: false,
                },
            )
            .unwrap();
        }
        write_peer_record(
            &pdir,
            &PeerRecord {
                hostname: "unit-eagle".into(),
                mde_version: Some("11.0.5".into()),
                last_seen_ms: now,
                health: "healthy".into(),
                descriptors: None,
                overlay_ip: Some("10.42.0.2".into()), // not in the LH roster
                role: None,
                external_addr: None,
                media: false,
            },
        )
        .unwrap();

        // Seed TWO replicated bundles, exactly the divergent shapes that land on
        // a real mesh: a "skinny" self-only bundle (what the auto-signer /
        // mesh_init write — a single self entry) AND a full-roster bundle (what
        // the /enroll listener writes, LIGHTHOUSE-10). `lighthouse_overlay_ips`
        // must UNION them — a skinny bundle surfacing first under `read_dir` must
        // not shadow the full roster, or the second lighthouse (10.42.0.3) goes
        // missing and HA-4 under-counts again.
        let mk =
            |mesh_overlay: &str, lhs: Vec<(&str, &str, &str)>| crate::ca::bundle::NebulaBundle {
                mesh_id: "eagle".into(),
                epoch: 1,
                ca_cert_pem: "-----BEGIN NEBULA CA-----\n-----END NEBULA CA-----\n".into(),
                peer_cert_pem: "-----BEGIN NEBULA CERT-----\n-----END NEBULA CERT-----\n".into(),
                overlay_ip: mesh_overlay.into(),
                mesh_cidr: "10.42.0.0/16".into(),
                lighthouses: lhs
                    .into_iter()
                    .map(|(node_id, ip, ext)| crate::ca::bundle::LighthouseEntry {
                        node_id: node_id.into(),
                        overlay_ip: ip.into(),
                        external_addr: ext.into(),
                        relay_tls: None,
                    })
                    .collect(),
                relay_trust_authority: None,
                created_at: 1,
            };
        // Skinny: only lh-1 (the signer's hardcoded conventional first host).
        crate::ca::bundle::write_bundle(
            &crate::ca::bundle::bundle_path(root, "peer:unit-eagle"),
            &mk(
                "10.42.0.2",
                vec![("peer:lh-1", "10.42.0.1", "203.0.113.1:4242")],
            ),
        )
        .unwrap();
        // Full roster: both lighthouses.
        crate::ca::bundle::write_bundle(
            &crate::ca::bundle::bundle_path(root, "peer:lh-2"),
            &mk(
                "10.42.0.3",
                vec![
                    ("peer:lh-1", "10.42.0.1", "203.0.113.1:4242"),
                    ("peer:lh-2", "10.42.0.3", "203.0.113.3:4242"),
                ],
            ),
        )
        .unwrap();

        let svc = DirectoryService::new(root, None); // non-signer: no roster DB

        // The union must carry BOTH lighthouse IPs regardless of read order.
        let ips = svc.lighthouse_overlay_ips();
        assert!(
            ips.contains(&"10.42.0.1".to_string()) && ips.contains(&"10.42.0.3".to_string()),
            "union of skinny + full bundles must carry both lighthouses, got {ips:?}"
        );
        let dir = svc.build_directory(now);
        let role_of = |host: &str| -> String {
            dir["peers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["hostname"] == host)
                .map(|p| p["role"].as_str().unwrap_or("").to_string())
                .unwrap_or_default()
        };
        assert_eq!(role_of("lh-1"), "lighthouse", "roster IP ⇒ lighthouse");
        assert_eq!(role_of("lh-2"), "lighthouse", "roster IP ⇒ lighthouse");
        assert_eq!(role_of("unit-eagle"), "peer", "non-roster IP stays a peer");

        // The directory the panel reads and the healthz counter now AGREE: both
        // are built off `directory_row`, so the canonical roster resolves both.
        let (_t, _h, _d, _u, _l, lighthouses) = svc.mesh_health_counts("unit-eagle", now);
        assert_eq!(
            lighthouses, 2,
            "directory + healthz lighthouse_count cannot diverge"
        );
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
                overlay_ip: None,
                role: None,
                external_addr: None,
                media: false,
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
                overlay_ip: None,
                role: None,
                external_addr: None,
                media: false,
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
    fn retired_lifecycle_verb_never_writes_a_request_file() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DirectoryService::new(tmp.path(), None);
        let reply: serde_json::Value = serde_json::from_str(&lifecycle_reply(
            &svc,
            Some(r#"{"peer":"oak","kind":"container","name":"nginx","op":"start"}"#),
            "ulid-1",
        ))
        .unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("authenticated"));
        assert!(crate::lifecycle::take_requests(tmp.path(), "oak").is_empty());
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
            overlay_ip: None,
            role: None,
            external_addr: None,
            media: false,
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
