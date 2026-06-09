//! Shell settings store, served on the mesh **Bus** (E0.3.4).
//!
//! Migrated off the `dev.mackes.MDE.Settings` D-Bus `#[interface]`
//! (which was never registered — see the E0.3 registration audit)
//! onto the Bus action/reply pattern at `action/settings/<verb>`.
//! Verbs take their arguments in the request body — `get`'s key,
//! `set`'s key + value, `restore`'s snapshot json — and route
//! through the unchanged `crate::settings::{current, apply}` +
//! `SettingKey` / `Snapshot`. Registering the responder (mackesd
//! `run_serve`) makes the store genuinely reachable for the first
//! time.
//!
//! The old `changed` D-Bus signal is dropped — it was never emitted
//! (the service was never on a connection) and has no subscriber. A
//! change-notify lands as a Bus event topic if/when the applier path
//! needs to push reconcile updates (Phase-future).

#![cfg(feature = "async-services")]

use std::collections::HashMap;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// Settings service handle. Stateless — the verbs route through the
/// free fns in `crate::settings`; kept as the responder handle for
/// symmetry with the other ipc responders (and so a future store
/// handle can hang off it without changing call sites).
#[derive(Debug, Default, Clone)]
pub struct SettingsService;

/// Action verbs served on `action/settings/<verb>` (E0.3.4).
pub const ACTION_VERBS: [&str; 5] = ["get", "set", "list-keys", "snapshot", "restore"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for verb `verb`: `action/settings/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/settings/{verb}")
}

/// Build the reply body for one `action/settings/<verb>` request.
/// Arguments travel in `req_body`:
///   * `get`       — body is the dot-notated key; reply is the
///     JSON-encoded value.
///   * `set`       — body is `{"key": "...", "value_json": "..."}`;
///     reply is `{"ok": true}` or an error envelope.
///   * `list-keys` — no body; reply is a JSON `[String]` of keys.
///   * `snapshot`  — no body; reply is the JSON `Snapshot`.
///   * `restore`   — body is a snapshot JSON; reply is `{"ok": true}`
///     or an error envelope.
///
/// Any failure is an `{"error": "..."}` envelope so the caller can
/// surface a diagnostic rather than time out.
#[must_use]
pub fn build_reply(_svc: &SettingsService, verb: &str, req_body: Option<&str>) -> String {
    use crate::settings::{apply, current, SettingKey, SettingValue, Snapshot};
    let err = |m: String| json!({ "error": m }).to_string();
    match verb {
        "get" => {
            let Some(key_str) = req_body else {
                return err("get: missing key in request body".into());
            };
            let key: SettingKey = match key_str.trim().parse() {
                Ok(k) => k,
                Err(e) => return err(format!("get: {e}")),
            };
            match current(key) {
                Ok(v) => {
                    serde_json::to_string(&v).unwrap_or_else(|e| err(format!("get encode: {e}")))
                }
                Err(e) => err(format!("get: {e:#}")),
            }
        }
        "set" => {
            let Some(body) = req_body else {
                return err("set: missing request body".into());
            };
            let req: serde_json::Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => return err(format!("set: bad request json: {e}")),
            };
            let key_str = req
                .get("key")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let value_json = req
                .get("value_json")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let key: SettingKey = match key_str.parse() {
                Ok(k) => k,
                Err(e) => return err(format!("set: {e}")),
            };
            let value: SettingValue = match serde_json::from_str(value_json) {
                Ok(v) => v,
                Err(e) => return err(format!("set: value_json: {e}")),
            };
            match apply(key, &value) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("set: {e:#}")),
            }
        }
        "list-keys" => {
            let keys: Vec<&str> = SettingKey::all().iter().map(|k| k.as_str()).collect();
            serde_json::to_string(&keys).unwrap_or_else(|e| err(format!("list-keys encode: {e}")))
        }
        "snapshot" => {
            let mut snap = Snapshot::default();
            for &key in SettingKey::all() {
                if let Ok(v) = current(key) {
                    snap.values.insert(key.as_str().to_string(), v);
                }
            }
            snap.captured_at = Some(chrono::Utc::now());
            serde_json::to_string(&snap).unwrap_or_else(|e| err(format!("snapshot encode: {e}")))
        }
        "restore" => {
            let Some(body) = req_body else {
                return err("restore: missing snapshot json".into());
            };
            let snap: Snapshot = match serde_json::from_str(body) {
                Ok(s) => s,
                Err(e) => return err(format!("restore: snapshot json: {e}")),
            };
            for (key_str, value) in &snap.values {
                let key: SettingKey = match key_str.parse() {
                    Ok(k) => k,
                    Err(e) => return err(format!("restore: {key_str}: {e}")),
                };
                if let Err(e) = apply(key, value) {
                    return err(format!("restore: {key_str}: {e:#}"));
                }
            }
            json!({ "ok": true }).to_string()
        }
        other => err(format!("unknown settings verb: {other}")),
    }
}

/// Run the Settings Bus responder loop on the current thread until
/// `should_stop()`. No tokio runtime needed (the settings free fns
/// are synchronous; `Persist`/rusqlite isn't `Send`, so `mackesd`
/// `run_serve` spawns this on a dedicated OS thread — same shape as
/// the Shell + Fleet responders).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &SettingsService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out so a test can
/// drive it without the sleep loop). For each new request on
/// `action/settings/<verb>`, runs [`build_reply`] (passing the
/// request body for the arg-bearing verbs) and writes the reply to
/// `reply/<ulid>`.
pub fn poll_once(persist: &Persist, svc: &SettingsService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "settings responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = build_reply(svc, verb, msg.body.as_deref());
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "settings responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_verbs_and_topic_lock() {
        assert_eq!(
            ACTION_VERBS,
            ["get", "set", "list-keys", "snapshot", "restore"]
        );
        assert_eq!(action_topic("get"), "action/settings/get");
    }

    #[test]
    fn list_keys_reply_carries_every_setting_key() {
        let svc = SettingsService;
        let body = build_reply(&svc, "list-keys", None);
        let keys: Vec<String> = serde_json::from_str(&body).expect("list-keys json");
        assert_eq!(keys.len(), crate::settings::SettingKey::all().len());
        assert!(keys.iter().any(|k| k == "theme.accent"));
        assert!(keys.iter().any(|k| k == "power.profile"));
    }

    #[test]
    fn get_unknown_key_yields_error_envelope() {
        let svc = SettingsService;
        let body = build_reply(&svc, "get", Some("never.a.real.key"));
        assert!(body.contains("error"));
        assert!(body.contains("unknown setting key"));
    }

    #[test]
    fn set_malformed_value_json_yields_error_envelope() {
        let svc = SettingsService;
        let req = json!({ "key": "theme.name", "value_json": "{not json}" }).to_string();
        let body = build_reply(&svc, "set", Some(&req));
        assert!(body.contains("error"));
        assert!(body.contains("value_json"));
    }

    #[test]
    fn unknown_verb_yields_error_envelope() {
        let svc = SettingsService;
        assert!(build_reply(&svc, "bogus", None).contains("unknown settings verb"));
    }
}
