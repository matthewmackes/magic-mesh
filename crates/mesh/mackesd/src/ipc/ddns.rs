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
pub const ACTION_VERBS: [&str; 4] = ["get-config", "set-config", "add-record", "remove-record"];

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
        other => err(format!("unknown ddns verb: {other}")),
    }
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
}
