//! DDNS-EGRESS-3 (responder) — `action/ddns/*` over the `[ddns]` config.
//!
//! CRUD on the per-node [`mackes_mesh_types::ddns::DdnsConfig`] (TOML on the
//! shared substrate) so the GUI/CLI manage DDNS records; the `ddns` worker
//! (subscribe to VPN-GW exit-IP changes + the DigitalOcean `DnsWriter`) reconciles
//! against this config. Same dedicated-OS-thread shape as the Connect/VPN
//! responders.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use mackes_mesh_types::ddns::{self, DdnsConfig, RecordDef};

use crate::workers::ddns::{provider_for_source, DnsReconciler, DoDnsReconciler, EgressState};

/// The DDNS responder — rooted at the shared workgroup root (the config home).
/// Holds this node's id (for record templating + the `wan`/`tunnel:<id>` source
/// resolution) + the path the `ddns` worker persists its discovered egress IPs
/// to, so `record-status` can report each record's **current published IP** and
/// `sync-now` can reconcile against the latest discovered readings.
#[derive(Debug, Clone)]
pub struct DdnsService {
    workgroup_root: PathBuf,
    node_id: String,
    /// Where the `ddns` worker persists its last-seen egress readings
    /// (`/var/lib/mackesd/egress-ip.json` in production; a tempdir in tests).
    egress_state_path: PathBuf,
}

impl DdnsService {
    /// Build the service rooted at the shared workgroup root, for `node_id`
    /// (the same `{node}` the `ddns` worker templates records with — passed in
    /// from the daemon so the responder + worker agree byte-for-byte). The
    /// egress-state path defaults to the worker's canonical
    /// `/var/lib/mackesd/egress-ip.json`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: impl Into<String>) -> Self {
        Self {
            workgroup_root,
            node_id: node_id.into(),
            egress_state_path: PathBuf::from(crate::workers::ddns::DEFAULT_STATE_PATH),
        }
    }

    /// Override the egress-state path (tests point this at a fixture).
    #[must_use]
    pub fn with_egress_state_path(mut self, path: PathBuf) -> Self {
        self.egress_state_path = path;
        self
    }

    /// The current published reading for a record `source` (`wan` /
    /// `tunnel:<id>`), read from the worker's persisted egress state. `None`
    /// when nothing has been discovered/published for that source yet.
    fn published_reading(&self, source: &str) -> Option<crate::workers::ddns::EgressReading> {
        EgressState::load(&self.egress_state_path)
            .last
            .get(source)
            .cloned()
    }
}

/// Action verbs served on `action/ddns/<verb>`. DDNS-EGRESS-3 adds the
/// record-level read/sync surface on top of the config CRUD: `list-records`
/// (each record + its current published IP), `record-status` (one record's
/// resolved FQDN + current published IP + source-up), and `sync-now` (reconcile
/// every record against the latest discovered egress readings, now).
pub const ACTION_VERBS: [&str; 7] = [
    "get-config",
    "set-config",
    "add-record",
    "remove-record",
    "list-records",
    "record-status",
    "sync-now",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/ddns/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/ddns/{verb}")
}

/// Build the reply for one `action/ddns/<verb>` request.
#[must_use]
pub fn build_reply(svc: &DdnsService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    match verb {
        "get-config" => {
            let cfg = ddns::load(root);
            json!({ "ok": true, "config": cfg }).to_string()
        }
        "set-config" => {
            let Some(body) = req_body else {
                return err("set-config: missing DdnsConfig body".into());
            };
            let cfg: DdnsConfig = match serde_json::from_str(body) {
                Ok(c) => c,
                Err(e) => return err(format!("set-config: bad json: {e}")),
            };
            match ddns::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("set-config: save: {e}")),
            }
        }
        "add-record" => {
            let Some(body) = req_body else {
                return err("add-record: missing RecordDef body".into());
            };
            let rec: RecordDef = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(e) => return err(format!("add-record: bad json: {e}")),
            };
            if rec.name.trim().is_empty() || rec.source.trim().is_empty() {
                return err("add-record: name and source are required".into());
            }
            let mut cfg = ddns::load(root);
            // Upsert by name template (the stable key).
            if let Some(e) = cfg.record.iter_mut().find(|r| r.name == rec.name) {
                *e = rec;
            } else {
                cfg.record.push(rec);
            }
            match ddns::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("add-record: save: {e}")),
            }
        }
        "remove-record" => {
            let Some(name) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("remove-record: missing record name".into());
            };
            let mut cfg = ddns::load(root);
            let before = cfg.record.len();
            cfg.record.retain(|r| r.name != name);
            if cfg.record.len() == before {
                return err(format!("remove-record: no record named '{name}'"));
            }
            match ddns::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("remove-record: save: {e}")),
            }
        }
        // DDNS-EGRESS-3 — each managed record + its current published IP (read
        // from the worker's persisted egress state) so the config + the
        // auto-added records are inspectable over the typed Bus IPC (§9 W27).
        "list-records" => {
            let cfg = ddns::load(root);
            let records: Vec<serde_json::Value> = cfg
                .record
                .iter()
                .map(|r| record_view(svc, &cfg, r))
                .collect();
            json!({ "ok": true, "enabled": cfg.enabled, "zone": cfg.zone, "records": records })
                .to_string()
        }
        // DDNS-EGRESS-3 — one record's resolved FQDN + current published IP +
        // whether its source is currently up. Body is the record name template.
        "record-status" => {
            let Some(name) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("record-status: missing record name".into());
            };
            let cfg = ddns::load(root);
            let Some(rec) = cfg.record.iter().find(|r| r.name == name) else {
                return err(format!("record-status: no record named '{name}'"));
            };
            json!({ "ok": true, "record": record_view(svc, &cfg, rec) }).to_string()
        }
        // DDNS-EGRESS-3 — reconcile every managed record against the latest
        // discovered egress reading NOW (the operator's "Sync now"), driving the
        // same DigitalOcean DnsWriter the worker does on a change. Honest when
        // DDNS is disabled (no write) — never a silent no-op masquerading as a
        // sync. Returns the count of records reconciled.
        "sync-now" => build_sync_now_reply(svc),
        other => err(format!("unknown ddns verb: {other}")),
    }
}

/// DDNS-EGRESS-3 — the per-record view for `list-records`/`record-status`: the
/// resolved FQDN (templated with this node + the source's provider), the source
/// key, the `on_down` policy, and the **current published IP** (the worker's
/// last-seen reading for that source) + whether the source is currently up.
#[must_use]
fn record_view(svc: &DdnsService, cfg: &DdnsConfig, rec: &RecordDef) -> serde_json::Value {
    let provider = provider_for_source(&rec.source);
    let fqdn = rec.fqdn(&svc.node_id, provider, 1, &cfg.zone);
    let reading = svc.published_reading(&rec.source);
    let (v4, v6, up) = match &reading {
        Some(r) => (r.v4.clone(), r.v6.clone(), !r.is_empty()),
        None => (None, None, false),
    };
    json!({
        "name": rec.name,
        "source": rec.source,
        "on_down": rec.on_down,
        "fqdn": fqdn,
        "current_ip_v4": v4,
        "current_ip_v6": v6,
        "source_up": up,
    })
}

/// DDNS-EGRESS-3 — drive `sync-now`: for every distinct record source, reconcile
/// the records bound to it against the worker's last-discovered reading via the
/// production [`DoDnsReconciler`] (the same writer path a change takes). When
/// DDNS is disabled the reconciler honestly does nothing (0 records) rather than
/// pretending to sync.
#[must_use]
fn build_sync_now_reply(svc: &DdnsService) -> String {
    let root = svc.workgroup_root.as_path();
    let cfg = ddns::load(root);
    if !cfg.enabled {
        return json!({ "ok": true, "enabled": false, "reconciled": 0,
            "detail": "ddns disabled; nothing synced" })
        .to_string();
    }
    let reconciler = DoDnsReconciler::new(svc.workgroup_root.clone(), svc.node_id.clone());
    let state = EgressState::load(&svc.egress_state_path);
    // Distinct source keys in config order (dedup, preserve first-seen order).
    let mut sources: Vec<&str> = Vec::new();
    for r in &cfg.record {
        if !sources.contains(&r.source.as_str()) {
            sources.push(&r.source);
        }
    }
    let mut reconciled = 0usize;
    for source in sources {
        // The latest reading for this source (empty when nothing discovered yet
        // / the source is down → the reconciler applies the on_down policy).
        let reading = state.last.get(source).cloned().unwrap_or_default();
        reconciled += reconciler.reconcile(source, &reading);
    }
    json!({ "ok": true, "enabled": true, "reconciled": reconciled }).to_string()
}

/// Run the DDNS Bus responder loop until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DdnsService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &DdnsService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "ddns responder: list_since failed");
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "ddns responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> (tempfile::TempDir, DdnsService) {
        let tmp = tempfile::tempdir().unwrap();
        // Point the egress-state path at a per-test file under the tempdir so
        // record-status/sync-now never read the host's real worker state.
        let s = DdnsService::new(tmp.path().to_path_buf(), "eagle")
            .with_egress_state_path(tmp.path().join("egress-ip.json"));
        (tmp, s)
    }

    #[test]
    fn action_verbs_lock() {
        assert_eq!(action_topic("sync-now"), "action/ddns/sync-now");
        assert_eq!(ACTION_VERBS.len(), 7);
        for v in ["get-config", "set-config", "add-record", "remove-record"] {
            assert!(ACTION_VERBS.contains(&v), "{v}");
        }
        // DDNS-EGRESS-3 record-level surface.
        assert!(ACTION_VERBS.contains(&"list-records"));
        assert!(ACTION_VERBS.contains(&"record-status"));
        assert!(ACTION_VERBS.contains(&"sync-now"));
    }

    #[test]
    fn get_config_returns_defaults() {
        let (_t, s) = service();
        let r = build_reply(&s, "get-config", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert_eq!(v["config"]["provider"], "digitalocean");
        assert_eq!(v["config"]["enabled"], serde_json::Value::Bool(false));
    }

    #[test]
    fn add_get_remove_record_round_trip() {
        let (_t, s) = service();
        let add = build_reply(
            &s,
            "add-record",
            Some(&json!({"name":"{node}-{provider}","source":"wan","on_down":"keep"}).to_string()),
        );
        assert!(add.contains("\"ok\":true"), "{add}");
        let cfg = build_reply(&s, "get-config", None);
        assert!(cfg.contains("{node}-{provider}"), "{cfg}");
        let rm = build_reply(&s, "remove-record", Some("{node}-{provider}"));
        assert!(rm.contains("\"ok\":true"), "{rm}");
        assert!(build_reply(&s, "remove-record", Some("ghost")).contains("no record named"));
    }

    #[test]
    fn set_config_persists() {
        let (_t, s) = service();
        let body = json!({"enabled":true,"provider":"digitalocean","zone":"z.example",
                          "token_ref":"secret:do","ttl":30,"record":[]})
        .to_string();
        assert!(build_reply(&s, "set-config", Some(&body)).contains("\"ok\":true"));
        let cfg = build_reply(&s, "get-config", None);
        assert!(cfg.contains("z.example") && cfg.contains("\"ttl\":30"));
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let (_t, s) = service();
        assert!(build_reply(&s, "bogus", None).contains("unknown ddns verb"));
        assert!(build_reply(&s, "add-record", None).contains("missing RecordDef"));
    }

    // ── DDNS-EGRESS-3 — list-records / record-status / sync-now ───────────────

    /// Seed the worker's persisted egress state with a reading for `source`,
    /// merging into any already-seeded sources (so multiple calls accumulate).
    fn seed_egress(svc: &DdnsService, source: &str, v4: Option<&str>, v6: Option<&str>) {
        let mut st = crate::workers::ddns::EgressState::load(&svc.egress_state_path);
        st.last.insert(
            source.to_owned(),
            crate::workers::ddns::EgressReading {
                v4: v4.map(str::to_owned),
                v6: v6.map(str::to_owned),
            },
        );
        st.store(&svc.egress_state_path).unwrap();
    }

    #[test]
    fn list_records_reports_each_record_with_its_published_ip() {
        let (_t, s) = service();
        // A wan record + a tunnel record.
        assert!(build_reply(
            &s,
            "add-record",
            Some(&json!({"name":"{node}-wan","source":"wan","on_down":"keep"}).to_string()),
        )
        .contains("\"ok\":true"));
        assert!(build_reply(
            &s,
            "add-record",
            Some(
                &json!({"name":"{node}-{provider}","source":"tunnel:mullvad-1","on_down":"remove"})
                    .to_string()
            ),
        )
        .contains("\"ok\":true"));
        // The worker has discovered a WAN IP + a tunnel exit IP.
        seed_egress(&s, "wan", Some("203.0.113.7"), None);
        seed_egress(&s, "tunnel:mullvad-1", Some("185.65.1.1"), None);

        let r = build_reply(&s, "list-records", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        let recs = v["records"].as_array().unwrap();
        assert_eq!(recs.len(), 2);
        // WAN record resolves with `wan` as the provider value + shows the raw IP.
        let wan = recs.iter().find(|r| r["source"] == "wan").unwrap();
        assert_eq!(wan["fqdn"], "eagle-wan.services.matthewmackes.com");
        assert_eq!(wan["current_ip_v4"], "203.0.113.7");
        assert_eq!(wan["source_up"], serde_json::Value::Bool(true));
        // Tunnel record: provider value strips the `tunnel:` prefix.
        let t = recs
            .iter()
            .find(|r| r["source"] == "tunnel:mullvad-1")
            .unwrap();
        assert_eq!(t["fqdn"], "eagle-mullvad-1.services.matthewmackes.com");
        assert_eq!(t["current_ip_v4"], "185.65.1.1");
    }

    #[test]
    fn wan_record_publishes_the_raw_wan_ip() {
        // DDNS-EGRESS-3 acceptance: a source="wan" record resolves through to the
        // raw WAN IP the worker discovered (no tunnel involved).
        let (_t, s) = service();
        seed_egress(&s, "wan", Some("198.51.100.9"), Some("2001:db8::1"));
        assert!(build_reply(
            &s,
            "add-record",
            Some(&json!({"name":"{node}-wan","source":"wan"}).to_string()),
        )
        .contains("\"ok\":true"));
        let r = build_reply(&s, "record-status", Some("{node}-wan"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["record"]["current_ip_v4"], "198.51.100.9");
        assert_eq!(v["record"]["current_ip_v6"], "2001:db8::1");
        assert_eq!(v["record"]["source_up"], serde_json::Value::Bool(true));
    }

    #[test]
    fn record_status_for_an_undiscovered_source_is_down_not_an_error() {
        let (_t, s) = service();
        assert!(build_reply(
            &s,
            "add-record",
            Some(&json!({"name":"{node}-{provider}","source":"tunnel:proton-2"}).to_string()),
        )
        .contains("\"ok\":true"));
        // Nothing discovered yet → source_up false, no published IP, no error.
        let r = build_reply(&s, "record-status", Some("{node}-{provider}"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert_eq!(v["record"]["source_up"], serde_json::Value::Bool(false));
        assert_eq!(v["record"]["current_ip_v4"], serde_json::Value::Null);
        // A ghost record errors honestly.
        assert!(build_reply(&s, "record-status", Some("ghost")).contains("no record named"));
        assert!(build_reply(&s, "record-status", None).contains("missing record name"));
    }

    #[test]
    fn sync_now_is_honest_when_disabled() {
        let (_t, s) = service();
        // Default config is disabled → sync-now does nothing, honestly (not a
        // silent no-op pretending success with writes).
        let r = build_reply(&s, "sync-now", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert_eq!(v["enabled"], serde_json::Value::Bool(false));
        assert_eq!(v["reconciled"], 0);
    }

    #[test]
    fn sync_now_reconciles_when_enabled_unknown_provider_is_zero() {
        let (_t, s) = service();
        // Enable with a non-DO provider → the reconciler returns 0 (the DO
        // adapter is the only writer; an unknown provider is a clean no-write,
        // not a panic). Proves sync-now drives the reconciler runtime-reachably.
        let body = json!({"enabled":true,"provider":"route53","zone":"z.example",
                          "token_ref":"","ttl":60,
                          "record":[{"name":"{node}-wan","source":"wan","on_down":"keep"}]})
        .to_string();
        assert!(build_reply(&s, "set-config", Some(&body)).contains("\"ok\":true"));
        seed_egress(&s, "wan", Some("203.0.113.7"), None);
        let r = build_reply(&s, "sync-now", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert_eq!(v["enabled"], serde_json::Value::Bool(true));
        assert_eq!(v["reconciled"], 0);
    }
}
