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

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

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

/// Stable consumer scope for settings mutations. Settings are local to the
/// responder's desktop session, so capabilities bind to one closed node
/// scope rather than accepting a caller-selected target host.
const SETTINGS_ACTION_NODE_SCOPE: &str = "settings";

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

/// Parse a privileged settings mutation into its stable capability target and
/// the body consumed by the legacy handler. The original request remains the
/// exact-body authorization input; auth metadata is stripped only after the
/// capability has been verified. Parsing here is deliberately limited to
/// request-envelope shape/key validation and performs no settings I/O.
fn mutation_request(
    verb: &str,
    req_body: Option<&str>,
) -> Result<Option<(String, String)>, String> {
    if !matches!(verb, "set" | "restore") {
        return Ok(None);
    }
    let body = req_body.ok_or_else(|| format!("{verb}: missing request body"))?;
    let mut request: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| format!("{verb}: request body must be a JSON object"))?;
    let object = request
        .as_object_mut()
        .ok_or_else(|| format!("{verb}: request body must be a JSON object"))?;
    object.remove("schema_version");
    object.remove("armed_token");

    let (target, handler_body) = match verb {
        "set" => {
            let key = request
                .get("key")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "set: missing `key`".to_string())?;
            let key: crate::settings::SettingKey =
                key.parse().map_err(|error| format!("set: {error}"))?;
            (format!("setting:{}", key.as_str()), request.to_string())
        }
        "restore" => ("snapshot".to_string(), request.to_string()),
        _ => unreachable!("the privileged settings verb set is closed"),
    };
    Ok(Some((target, handler_body)))
}

/// Apply the shared-Bus authorization boundary before a settings mutation
/// can invoke an applier. Read-only settings verbs intentionally remain open.
fn build_authorized_reply(
    svc: &SettingsService,
    verb: &str,
    req_body: Option<&str>,
    authorizer: &ActionAuthorizer,
) -> String {
    let prepared = match mutation_request(verb, req_body) {
        Ok(prepared) => prepared,
        Err(error) => return json!({ "error": error }).to_string(),
    };
    let Some((target, handler_body)) = prepared else {
        return build_reply(svc, verb, req_body);
    };
    let auth_verb = format!("settings-{verb}");
    let context = MutationContext {
        verb: &auth_verb,
        node: SETTINGS_ACTION_NODE_SCOPE,
        target: &target,
    };
    let Some(body) = req_body else {
        unreachable!("mutation_request requires a request body")
    };
    if let Err(error) = authorizer.authorize(body, context) {
        tracing::warn!(
            target: "mackesd::action_auth",
            verb,
            %error,
            "refused unauthorized Settings mutation"
        );
        return json!({ "error": format!("{verb}: authorization refused: {error}") }).to_string();
    }
    build_reply(svc, verb, Some(&handler_body))
}

/// Run the Settings Bus responder loop on the current thread until
/// `should_stop()`. No tokio runtime needed (the settings free fns
/// are synchronous; `Persist`/rusqlite isn't `Send`, so `mackesd`
/// `run_serve` spawns this on a dedicated OS thread — same shape as
/// the Shell + Fleet responders).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &SettingsService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    let authorizer = ActionAuthorizer::production();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors, &authorizer);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out so a test can
/// drive it without the sleep loop). For each new request on
/// `action/settings/<verb>`, authenticates mutating requests, runs the
/// legacy [`build_reply`] handler (passing the request body for the
/// arg-bearing verbs), and writes the reply to `reply/<ulid>`.
pub fn poll_once(
    persist: &Persist,
    svc: &SettingsService,
    cursors: &mut HashMap<String, String>,
    authorizer: &ActionAuthorizer,
) {
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
            // EFF-23 — refuse an oversized body before build_reply parses it.
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_authorized_reply(svc, verb, msg.body.as_deref(), authorizer)
            } else {
                tracing::warn!(
                    topic = %topic,
                    len = msg.body.as_ref().map_or(0, String::len),
                    cap = crate::ipc::MAX_RPC_BODY_BYTES,
                    "settings responder: body exceeds cap; refusing",
                );
                crate::ipc::body_too_large_reply(verb)
            };
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
    use crate::ipc::action_auth::authorize_test_body;

    const AUTH_KEY: &[u8] = b"settings-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn context(verb: &'static str, target: &'static str) -> MutationContext<'static> {
        MutationContext {
            verb,
            node: SETTINGS_ACTION_NODE_SCOPE,
            target,
        }
    }

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
    fn hostile_unsigned_mutations_are_refused_before_settings_io() {
        let tmp = tempfile::tempdir().unwrap();
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW);
        let svc = SettingsService;
        let requests = [
            (
                "set",
                json!({
                    "schema_version": 1,
                    "key": "keyboard.repeat_delay",
                    "value_json": "{not json}"
                })
                .to_string(),
            ),
            (
                "restore",
                json!({ "schema_version": 1, "values": [] }).to_string(),
            ),
        ];
        for (verb, body) in requests {
            let reply = build_authorized_reply(&svc, verb, Some(&body), &authorizer);
            assert!(
                reply.contains("authorization refused"),
                "unsigned {verb} reached its handler: {reply}"
            );
        }
    }

    #[test]
    fn authorized_settings_mutation_is_exact_body_bound_and_single_use() {
        let tmp = tempfile::tempdir().unwrap();
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW);
        let svc = SettingsService;

        // A tampered set body must fail exact-body verification without
        // consuming the nonce; the original body then reaches the handler.
        let unsigned_set = json!({
            "schema_version": 1,
            "key": "keyboard.repeat_delay",
            "value_json": "{not json}"
        })
        .to_string();
        let set_context = context("settings-set", "setting:keyboard.repeat_delay");
        let armed_set = authorize_test_body(
            AUTH_KEY,
            &unsigned_set,
            set_context,
            "settings-tamper",
            AUTH_NOW + 30_000,
        );
        let tampered_set = armed_set.replace("{not json}", "different");
        assert!(
            build_authorized_reply(&svc, "set", Some(&tampered_set), &authorizer)
                .contains("authorization refused")
        );
        let accepted_set = build_authorized_reply(&svc, "set", Some(&armed_set), &authorizer);
        assert!(accepted_set.contains("value_json"), "{accepted_set}");

        // Restore has a harmless empty snapshot, so an authorized request can
        // complete while still proving that the mutation gate is exercised.
        let unsigned_restore = json!({ "schema_version": 1, "values": {} }).to_string();
        let restore_context = context("settings-restore", "snapshot");
        let armed_restore = authorize_test_body(
            AUTH_KEY,
            &unsigned_restore,
            restore_context,
            "settings-replay",
            AUTH_NOW + 30_000,
        );
        let first = build_authorized_reply(&svc, "restore", Some(&armed_restore), &authorizer);
        assert!(first.contains("\"ok\":true"), "{first}");
        let replay = build_authorized_reply(&svc, "restore", Some(&armed_restore), &authorizer);
        assert!(replay.contains("already used"), "{replay}");
    }

    #[test]
    fn unknown_verb_yields_error_envelope() {
        let svc = SettingsService;
        assert!(build_reply(&svc, "bogus", None).contains("unknown settings verb"));
    }
}
