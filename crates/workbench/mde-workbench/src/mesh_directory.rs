//! SUBSTRATE-8 (SUBSTRATE-V2) — read the mesh peer directory over the Bus RPC.
//!
//! The panels used to read the peer roster straight off `/mnt/mesh-storage`
//! (`peers::read_peers`). Post-cutover that path is a Syncthing folder and the
//! peer records live in etcd, so a direct-FS read returns nothing. This helper
//! routes every panel through `action/mesh/directory` instead — whose mackesd
//! responder reads etcd-or-fs (the SUBSTRATE-3 bridge), so the panels become
//! substrate-agnostic and keep working across the cutover. `mackes_mesh_types::
//! peers::PeerRecord` stays the in-GUI shape, rebuilt from the RPC reply.

use std::time::Duration;

use mackes_mesh_types::peers::PeerRecord;

/// Read budget for the directory probe — matches the peers panel's 2 s.
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(2);

/// Parse an `action/mesh/directory` reply (`{ ok, peers: [...] }`) into
/// `PeerRecord`s — the fields the panels render (hostname, overlay IP, health,
/// role, last-seen, version). Pure + testable. A non-ok / unparseable reply, or
/// a missing `peers` array, yields an empty list (an honest empty directory).
#[must_use]
pub fn parse_directory_peers(reply: &str) -> Vec<PeerRecord> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply.trim()) else {
        return Vec::new();
    };
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let Some(peers) = v.get("peers").and_then(|p| p.as_array()) else {
        return Vec::new();
    };
    peers
        .iter()
        .filter_map(|p| {
            let hostname = p.get("hostname").and_then(serde_json::Value::as_str)?;
            if hostname.is_empty() {
                return None;
            }
            let mut rec = PeerRecord::now(
                hostname,
                p.get("mde_version")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                p.get("health")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown"),
            );
            rec.last_seen_ms = p
                .get("last_seen_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(rec.last_seen_ms);
            rec.overlay_ip = p
                .get("overlay_ip")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            rec.role = p
                .get("role")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            Some(rec)
        })
        .collect()
}

/// Fetch the live peer directory over the Bus (`action/mesh/directory`). Empty on
/// a daemon-down / timeout / no-responder (the panels render an honest empty
/// state). Blocking — call from `tokio::task::spawn_blocking` on the iced
/// executor (the [`crate::dbus::action_request`] current-thread-runtime contract).
#[must_use]
pub fn fetch_peers() -> Vec<PeerRecord> {
    match crate::dbus::action_request("action/mesh/directory", DIRECTORY_TIMEOUT) {
        Some(reply) => parse_directory_peers(&reply),
        None => Vec::new(),
    }
}

/// Parse the leader hostname from an `action/mesh/directory` reply (`{ ok, …,
/// leader }`). `None` when there's no live leader / the reply is non-ok /
/// unparseable. Pure + testable (SUBSTRATE-8 — replaces the panels' direct
/// `.mackesd-leader.lock` reads).
#[must_use]
pub fn parse_directory_leader(reply: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    v.get("leader")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Fetch the current mesh leader's hostname over the Bus. `None` on daemon-down
/// / no live leader. Blocking — same contract as [`fetch_peers`].
#[must_use]
pub fn fetch_leader() -> Option<String> {
    parse_directory_leader(&crate::dbus::action_request(
        "action/mesh/directory",
        DIRECTORY_TIMEOUT,
    )?)
}

/// Fetch peers + leader in one directory round-trip (the panels that render both
/// — lighthouses, the notify-center footer — want a single RPC, not two).
#[must_use]
pub fn fetch_peers_and_leader() -> (Vec<PeerRecord>, Option<String>) {
    match crate::dbus::action_request("action/mesh/directory", DIRECTORY_TIMEOUT) {
        Some(reply) => (
            parse_directory_peers(&reply),
            parse_directory_leader(&reply),
        ),
        None => (Vec::new(), None),
    }
}

/// Parse the raw encoded leader lease (`node_id\trenewed_at_s\tepoch`) from a
/// directory reply — for Mesh Control, which renders the epoch/age. Pure.
#[must_use]
pub fn parse_directory_leader_lease(reply: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    v.get("leader_lease")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// Fetch the raw leader lease over the Bus. `None` on daemon-down / no leader.
#[must_use]
pub fn fetch_leader_lease() -> Option<String> {
    parse_directory_leader_lease(&crate::dbus::action_request(
        "action/mesh/directory",
        DIRECTORY_TIMEOUT,
    )?)
}

// ---- HA / healthz health-report read (HA-5) -------------------

/// Bus topic mackesd serves the `HealthReport` JSON line on
/// (`crates/mesh/mackesd/src/ipc/shell.rs`, `action/shell/<verb>`). The
/// daemon-side healthz is the only view that carries the live mesh-enriched
/// `lighthouse_count` / `ha_ok` (the CLI's store-only view reports 0).
const HEALTHZ_TOPIC: &str = "action/shell/healthz";

/// HA-5 — the HA-relevant subset of mackesd's `HealthReport`
/// (`crates/mesh/mackesd/src/health.rs`), decoded over the Bus. Only the fields
/// the Mesh Control HA card renders: the SUBSTRATE-V2 etcd quorum runs on the
/// lighthouses, so `lighthouse_count` is both the lighthouse-HA count AND the
/// etcd member count; `ha_ok` is the daemon's own ≥2-lighthouse verdict; the
/// node buckets give the mesh-size context. Defaults are the honest
/// daemon-unreachable baseline (everything zero / `ha_ok=false`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthSummary {
    /// Total mesh size from the leader's directory view.
    pub node_count: u32,
    /// Nodes whose last heartbeat is within the healthy threshold.
    pub healthy_nodes: u32,
    /// Lighthouse count — the SUBSTRATE-V2 etcd quorum members + the
    /// relay/discovery anchors. Drives both the etcd-quorum verdict and the
    /// lighthouse-HA readiness in the card.
    pub lighthouse_count: u32,
    /// The daemon's own HA verdict (`lighthouse_count >= HA_MIN_LIGHTHOUSES`).
    /// `false` = single lighthouse / no failover headroom.
    pub ha_ok: bool,
}

/// Parse a mackesd `action/shell/healthz` reply (a raw `HealthReport` JSON line,
/// NOT a `{ok,…}` envelope — see `ipc::shell::build_reply`) into the
/// HA-relevant [`HealthSummary`]. `None` when the body is a `{"error":…}`
/// envelope or unparseable; missing numeric fields default to 0 / `false` so an
/// older daemon (pre-`lighthouse_count`) still decodes to an honest "no HA"
/// posture rather than failing the whole card. Pure + testable.
#[must_use]
pub fn parse_health_report(reply: &str) -> Option<HealthSummary> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    // Builder errors come back as `{"error": "..."}` — treat as unreachable.
    if v.get("error").is_some() {
        return None;
    }
    // Require at least one HealthReport-shaped field so we don't accept an
    // arbitrary unrelated JSON object as a (zeroed) report.
    if v.get("schema").is_none() && v.get("node_count").is_none() {
        return None;
    }
    let u32_field = |key: &str| -> u32 {
        v.get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    Some(HealthSummary {
        node_count: u32_field("node_count"),
        healthy_nodes: u32_field("healthy_nodes"),
        lighthouse_count: u32_field("lighthouse_count"),
        ha_ok: v
            .get("ha_ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// Fetch the live HA health summary over the Bus (`action/shell/healthz`).
/// `None` on daemon-down / timeout / no-responder (the card renders an honest
/// "daemon unreachable" state). Blocking — same current-thread-runtime contract
/// as [`fetch_peers`]: call from `tokio::task::spawn_blocking`.
#[must_use]
pub fn fetch_health() -> Option<HealthSummary> {
    parse_health_report(&crate::dbus::action_request(
        HEALTHZ_TOPIC,
        DIRECTORY_TIMEOUT,
    )?)
}

// ---- HA-5 — the published coordination-plane status (mesh/ha/status) -------

/// Retained Bus data lane the `ha_monitor` worker publishes the coordination-
/// plane snapshot to (`crates/mesh/mackesd/src/workers/ha_monitor.rs`,
/// [`HaStatusDoc`]). This is the authoritative *published* view the Mesh Control
/// panel consumes — distinct from the live healthz RPC: it's what the worker
/// observed + alerted on, so the panel can show the same member/quorum/leader
/// state the Alert Center fires against.
const HA_STATUS_TOPIC: &str = "mesh/ha/status";

/// HA-5 — the worker-published coordination-plane status the Mesh Control panel
/// consumes off [`HA_STATUS_TOPIC`]. Shape mirrors the daemon's `HaStatusDoc`
/// (member count + quorum-OK + leader), parsed by convention (no cross-crate
/// dep, exactly like [`HealthSummary`] ↔ the daemon's `HealthReport`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HaStatus {
    /// etcd-quorum member count (the lighthouse roster).
    pub member_count: u32,
    /// Whether a leader is elected (etcd quorum healthy).
    pub quorum_ok: bool,
    /// Current leader's bare hostname, or `None` when no leader is elected.
    pub leader: Option<String>,
}

/// Parse a `mesh/ha/status` message body (the worker's `HaStatusDoc` JSON) into
/// an [`HaStatus`]. `None` when the body is unparseable or carries none of the
/// expected fields (so an unrelated message can't decode to a zeroed status).
/// Pure + testable.
#[must_use]
pub fn parse_ha_status(body: &str) -> Option<HaStatus> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    if v.get("quorum_ok").is_none() && v.get("member_count").is_none() {
        return None;
    }
    Some(HaStatus {
        member_count: v
            .get("member_count")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        quorum_ok: v
            .get("quorum_ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        leader: v
            .get("leader")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// Fetch the latest worker-published HA status off the Bus. Reads the most
/// recent message on [`HA_STATUS_TOPIC`] (the `ha_monitor` publishes on change
/// at `high` priority, so the current-state row persists). `None` on no Bus
/// data-dir / persist error / an empty topic (the panel falls back to the live
/// healthz RPC — honest degradation). Blocking — same contract as
/// [`fetch_peers`]: call from `tokio::task::spawn_blocking`.
#[must_use]
pub fn fetch_ha_status() -> Option<HaStatus> {
    let bus_dir = mde_bus::client_data_dir()?;
    let persist = mde_bus::persist::Persist::open(bus_dir).ok()?;
    // The published topic is small (published on change); read it and take the
    // most recent message — the established "latest on a topic" read shape
    // (`crate::dbus::action_request_reply_on`).
    let latest = persist.list_since(HA_STATUS_TOPIC, None).ok()?.into_iter().next_back()?;
    parse_ha_status(latest.body.as_deref()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_directory_peers_decodes_the_rpc_shape() {
        // The exact shape mackesd's action/mesh/directory responder emits.
        let reply = r#"{"ok":true,"head":7,"peers":[
            {"hostname":"node-a","presence":"online","last_seen_ms":111,
             "health":"healthy","mde_version":"11.0.0","overlay_ip":"10.42.0.2",
             "role":"server","tags":[]},
            {"hostname":"node-b","presence":"offline","last_seen_ms":222,
             "health":"degraded","mde_version":null,"overlay_ip":"","role":null,"tags":[]}
        ]}"#;
        let peers = parse_directory_peers(reply);
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].hostname, "node-a");
        assert_eq!(peers[0].overlay_ip.as_deref(), Some("10.42.0.2"));
        assert_eq!(peers[0].health, "healthy");
        assert_eq!(peers[0].role.as_deref(), Some("server"));
        assert_eq!(peers[0].mde_version.as_deref(), Some("11.0.0"));
        assert_eq!(peers[0].last_seen_ms, 111);
        // Empty overlay_ip / null role → None (not Some("")).
        assert_eq!(peers[1].overlay_ip, None);
        assert_eq!(peers[1].role, None);
        assert_eq!(peers[1].health, "degraded");
    }

    #[test]
    fn parse_directory_peers_handles_error_and_garbage() {
        assert!(parse_directory_peers(r#"{"error":"boom"}"#).is_empty());
        assert!(parse_directory_peers(r#"{"ok":false}"#).is_empty());
        assert!(parse_directory_peers("not json").is_empty());
        assert!(parse_directory_peers(r#"{"ok":true}"#).is_empty()); // no peers key
        assert!(parse_directory_peers(r#"{"ok":true,"peers":[]}"#).is_empty());
    }

    #[test]
    fn parse_directory_peers_skips_rows_without_hostname() {
        let reply =
            r#"{"ok":true,"peers":[{"health":"healthy"},{"hostname":"x","health":"healthy"}]}"#;
        let peers = parse_directory_peers(reply);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].hostname, "x");
    }

    #[test]
    fn parse_directory_leader_reads_hostname() {
        let reply = r#"{"ok":true,"leader":"node-b","leader_lease":"peer:node-b\t1750000000\t3","peers":[]}"#;
        assert_eq!(parse_directory_leader(reply).as_deref(), Some("node-b"));
        // No leader / empty / not-ok → None.
        assert!(parse_directory_leader(r#"{"ok":true,"leader":null,"peers":[]}"#).is_none());
        assert!(parse_directory_leader(r#"{"ok":true,"leader":"","peers":[]}"#).is_none());
        assert!(parse_directory_leader(r#"{"ok":false}"#).is_none());
    }

    #[test]
    fn parse_directory_leader_lease_reads_raw_lease() {
        let reply = r#"{"ok":true,"leader":"node-b","leader_lease":"peer:node-b\t1750000000\t3","peers":[]}"#;
        assert_eq!(
            parse_directory_leader_lease(reply).as_deref(),
            Some("peer:node-b\t1750000000\t3")
        );
        assert!(parse_directory_leader_lease(r#"{"ok":true,"peers":[]}"#).is_none());
    }

    // ---- HA-5 health-report decode ---------------------------

    #[test]
    fn parse_health_report_decodes_the_healthz_line() {
        // The exact raw HealthReport JSON line mackesd's
        // action/shell/healthz responder emits (no {ok,…} envelope).
        let reply = r#"{"schema":1,"is_leader":true,"applied_revision":null,
            "node_count":4,"healthy_nodes":3,"degraded_nodes":1,"unreachable_nodes":0,
            "audit_chain_intact":true,"version":"11.0.6","workers_alive":5,
            "workers_total":5,"breaker_tripped":0,"ready":true,
            "lighthouse_count":3,"ha_ok":true}"#;
        let h = parse_health_report(reply).expect("decoded");
        assert_eq!(h.node_count, 4);
        assert_eq!(h.healthy_nodes, 3);
        assert_eq!(h.lighthouse_count, 3);
        assert!(h.ha_ok);
    }

    #[test]
    fn parse_health_report_decodes_degraded_no_ha() {
        // A single-lighthouse mesh: the daemon reports ha_ok=false.
        let reply = r#"{"schema":1,"node_count":2,"healthy_nodes":2,
            "lighthouse_count":1,"ha_ok":false}"#;
        let h = parse_health_report(reply).expect("decoded");
        assert_eq!(h.lighthouse_count, 1);
        assert!(!h.ha_ok);
    }

    #[test]
    fn parse_health_report_rejects_error_and_garbage() {
        // Builder errors come back as {"error":…} → unreachable.
        assert!(parse_health_report(r#"{"error":"healthz encode: boom"}"#).is_none());
        assert!(parse_health_report("not json").is_none());
        // An unrelated JSON object (no schema / node_count) is not a report.
        assert!(parse_health_report(r#"{"ok":true,"leader":"node-b"}"#).is_none());
    }

    #[test]
    fn parse_health_report_tolerates_missing_ha_fields() {
        // An older daemon predating lighthouse_count/ha_ok still decodes —
        // the missing HA fields default to the honest "no HA" baseline.
        let reply = r#"{"schema":1,"node_count":1,"healthy_nodes":1}"#;
        let h = parse_health_report(reply).expect("decoded");
        assert_eq!(h.node_count, 1);
        assert_eq!(h.lighthouse_count, 0);
        assert!(!h.ha_ok);
    }

    // ---- HA-5 published-status decode (mesh/ha/status) --------

    #[test]
    fn parse_ha_status_decodes_the_worker_doc() {
        // The exact HaStatusDoc JSON the ha_monitor publishes.
        let body = r#"{"member_count":3,"quorum_ok":true,"leader":"kiln","ts_unix_ms":1750000000000}"#;
        let s = parse_ha_status(body).expect("decoded");
        assert_eq!(s.member_count, 3);
        assert!(s.quorum_ok);
        assert_eq!(s.leader.as_deref(), Some("kiln"));
    }

    #[test]
    fn parse_ha_status_handles_no_leader_and_null() {
        let body = r#"{"member_count":3,"quorum_ok":false,"leader":null,"ts_unix_ms":1}"#;
        let s = parse_ha_status(body).expect("decoded");
        assert_eq!(s.member_count, 3);
        assert!(!s.quorum_ok);
        assert_eq!(s.leader, None);
    }

    #[test]
    fn parse_ha_status_rejects_garbage_and_unrelated_json() {
        assert!(parse_ha_status("not json").is_none());
        // An unrelated object with none of the expected fields is not a status.
        assert!(parse_ha_status(r#"{"ok":true,"leader":"x"}"#).is_none());
    }
}
