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

use mackes_mesh_types::ddns::{self, DdnsConfig, RecordDef, SourceState};

/// The DDNS responder — rooted at the shared workgroup root (the config home).
#[derive(Debug, Clone)]
pub struct DdnsService {
    workgroup_root: PathBuf,
}

impl DdnsService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/ddns/<verb>`.
///
/// `record-status` (DDNS-EGRESS-4) is the reconcile-decision query: given a
/// record name + the live source state, it returns the planned [`ddns::DdnsAction`]
/// (reconnect rewrite / on-down policy) + the [`ddns::Reachability`] flag.
pub const ACTION_VERBS: [&str; 5] = [
    "get-config",
    "set-config",
    "add-record",
    "remove-record",
    "record-status",
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
        "record-status" => {
            let Some(body) = req_body else {
                return err("record-status: missing {name,state[,last]} body".into());
            };
            let q: StatusQuery = match serde_json::from_str(body) {
                Ok(q) => q,
                Err(e) => return err(format!("record-status: bad json: {e}")),
            };
            let cfg = ddns::load(root);
            let Some(record) = cfg.record.iter().find(|r| r.name == q.name) else {
                return err(format!("record-status: no record named '{}'", q.name));
            };
            let action = ddns::plan_action(record, q.last.as_deref(), &q.state);
            let reach = ddns::reachability(&q.state);
            json!({
                "ok": true,
                "name": q.name,
                "on_down": record.on_down,
                "action": action,
                "reachability": reach,
                "reachability_label": reach.label(),
            })
            .to_string()
        }
        other => err(format!("unknown ddns verb: {other}")),
    }
}

/// DDNS-EGRESS-4 — the `record-status` request body: a managed record's `name`,
/// the live source `state` (from the VPN-GW exit-IP verifier / WAN check), and
/// the `last`-published value (omit = never published). The reply carries the
/// planned [`ddns::DdnsAction`] + the [`ddns::Reachability`] flag.
#[derive(serde::Deserialize)]
struct StatusQuery {
    name: String,
    state: SourceState,
    #[serde(default)]
    last: Option<String>,
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
        let s = DdnsService::new(tmp.path().to_path_buf());
        (tmp, s)
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

    // ── DDNS-EGRESS-4: record-status reconcile-decision query ──────────────

    fn add_keep_record(s: &DdnsService) {
        let add = build_reply(
            s,
            "add-record",
            Some(
                &json!({"name":"eagle-mullvad","source":"tunnel:mullvad-1","on_down":"keep"})
                    .to_string(),
            ),
        );
        assert!(add.contains("\"ok\":true"), "{add}");
    }

    #[test]
    fn record_status_reconnect_rewrite_and_reachability() {
        let (_t, s) = service();
        add_keep_record(&s);
        // Up with a NEW ip vs last → upsert (the reconnect rewrite); no port
        // forward → identity-only ("port-forward only").
        let body = json!({
            "name": "eagle-mullvad",
            "last": "1.2.3.4",
            "state": {"state":"up","ip":"5.6.7.8","port_forward":false}
        })
        .to_string();
        let r = build_reply(&s, "record-status", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert_eq!(v["action"]["action"], "upsert");
        assert_eq!(v["action"]["ip"], "5.6.7.8");
        assert_eq!(v["reachability"], "identity-only");
        assert_eq!(v["reachability_label"], "port-forward only");
    }

    #[test]
    fn record_status_down_keep_with_kill_switch_parks_sentinel() {
        let (_t, s) = service();
        add_keep_record(&s);
        // Down + kill-switch engaged → leak-coupling parks at the sentinel.
        let body = json!({
            "name": "eagle-mullvad",
            "last": "1.2.3.4",
            "state": {"state":"down","kill_switch":true}
        })
        .to_string();
        let r = build_reply(&s, "record-status", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["action"]["action"], "upsert");
        assert_eq!(v["action"]["ip"], mackes_mesh_types::ddns::SENTINEL_ADDR);
        assert_eq!(v["reachability"], "down");
    }

    #[test]
    fn record_status_unknown_record_and_bad_body_error() {
        let (_t, s) = service();
        assert!(build_reply(&s, "record-status", None).contains("missing"));
        let body = json!({"name":"ghost","state":{"state":"down"}}).to_string();
        assert!(build_reply(&s, "record-status", Some(&body)).contains("no record named 'ghost'"));
    }
}
