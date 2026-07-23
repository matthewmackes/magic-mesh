//! CLIP-SYNC-1 (action layer) — the Bus responder for the mesh-global
//! clipboard, on `action/clipboard/<verb>` → `reply/<ulid>`. The Clipboard
//! Viewer (CLIP-VIEW-1) renders through these verbs; the clipboard_sync
//! worker owns the capture/append side. Both edit the SAME shared file
//! (`<root>/clipboard/history.json`) so a viewer edit is mesh-wide.
//!
//! Verbs (design `docs/design/notify-hub-redesign.md` O5/O7):
//!   * `list`   — no body; reply `{ "ok": true, "entries": [ClipEntry] }`.
//!   * `pin`    — body is an authorized JSON envelope carrying an entry id;
//!     O7 mark it pinned (exempt
//!     from the 50-cap + a clear, unlimited).
//!   * `unpin`  — authorized JSON entry id; clear the pin (re-subject to the cap).
//!   * `delete` — authorized JSON entry id; drop that one entry mesh-wide.
//!   * `clear`  — authorized JSON body; O5 mesh-wide clear — drop every UNPINNED entry,
//!     pinned survive everywhere.
//!
//! Same dedicated-OS-thread responder shape as Connect/Route (the history
//! free fns are synchronous; `Persist`/rusqlite isn't `Send`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use super::action_auth::{ActionAuthorizer, MutationContext};

use crate::workers::clipboard_sync::{
    history_path, read_history, trim_unpinned, write_history, History, HISTORY_CAP,
};

/// The clipboard responder service — holds the replicated workgroup root
/// where the shared history lives.
#[derive(Debug, Clone)]
pub struct ClipboardService {
    workgroup_root: PathBuf,
    authorizer: Arc<ActionAuthorizer>,
}

impl ClipboardService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject an isolated verifier and replay ledger for hostile responder tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }
}

/// Action verbs served on `action/clipboard/<verb>`.
pub const ACTION_VERBS: [&str; 5] = ["list", "pin", "unpin", "delete", "clear"];

/// Responder poll interval (matches the Connect responder cadence).
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for verb `verb`: `action/clipboard/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/clipboard/{verb}")
}

/// O7 — set/clear an entry's pin in `history`. Returns whether anything
/// changed. After unpinning, the now-uncapped entry is re-subjected to the
/// 50-cap (an unpinned 51st entry can be trimmed). Pure + testable.
#[must_use]
pub fn set_pin(history: &mut History, id: &str, pinned: bool) -> bool {
    let Some(e) = history.entries.iter_mut().find(|e| e.id == id) else {
        return false;
    };
    if e.pinned == pinned {
        return false;
    }
    e.pinned = pinned;
    if !pinned {
        trim_unpinned(history, HISTORY_CAP);
    }
    true
}

/// Drop the entry with `id` (any pin state). Returns whether it existed.
#[must_use]
pub fn delete_entry(history: &mut History, id: &str) -> bool {
    let before = history.entries.len();
    history.entries.retain(|e| e.id != id);
    history.entries.len() != before
}

/// O5 — mesh-wide clear: drop every unpinned entry, pinned survive.
/// Returns the number dropped.
pub fn clear_unpinned(history: &mut History) -> usize {
    let before = history.entries.len();
    history.entries.retain(|e| e.pinned);
    before - history.entries.len()
}

/// Build the reply body for one `action/clipboard/<verb>` request. Mutations
/// load → mutate → atomic write-through; any failure is an `{"error": …}`
/// envelope so the caller surfaces a diagnostic.
#[must_use]
pub fn build_reply(svc: &ClipboardService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let path = history_path(&svc.workgroup_root);
    match verb {
        "list" => {
            let h = read_history(&path);
            json!({ "ok": true, "entries": h.entries }).to_string()
        }
        "pin" | "unpin" => {
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err(format!("{verb}: missing entry id"));
            };
            let mut h = read_history(&path);
            if !set_pin(&mut h, id, verb == "pin") {
                // No such id, or already in the requested state — idempotent ok.
                return json!({ "ok": true, "changed": false }).to_string();
            }
            match write_history(&path, &h) {
                Ok(()) => json!({ "ok": true, "changed": true }).to_string(),
                Err(e) => err(format!("{verb}: save: {e}")),
            }
        }
        "delete" => {
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("delete: missing entry id".into());
            };
            let mut h = read_history(&path);
            if !delete_entry(&mut h, id) {
                return json!({ "ok": true, "changed": false }).to_string();
            }
            match write_history(&path, &h) {
                Ok(()) => json!({ "ok": true, "changed": true }).to_string(),
                Err(e) => err(format!("delete: save: {e}")),
            }
        }
        "clear" => {
            let mut h = read_history(&path);
            let dropped = clear_unpinned(&mut h);
            match write_history(&path, &h) {
                Ok(()) => json!({ "ok": true, "cleared": dropped }).to_string(),
                Err(e) => err(format!("clear: save: {e}")),
            }
        }
        other => err(format!("unknown clipboard verb: {other}")),
    }
}

/// Prepare a mutating clipboard request for the legacy pure handler. The
/// original JSON body remains the exact-body authorization input; only the
/// handler-facing entry id is returned after authorization succeeds.
fn mutation_request(
    verb: &str,
    req_body: Option<&str>,
) -> Result<(String, Option<String>), String> {
    if !matches!(verb, "pin" | "unpin" | "delete" | "clear") {
        return Err(format!("{verb}: not a clipboard mutation"));
    }
    let body = req_body.ok_or_else(|| format!("{verb}: missing request body"))?;
    let request: serde_json::Value =
        serde_json::from_str(body).map_err(|_| format!("{verb}: request body must be JSON"))?;
    let object = request
        .as_object()
        .ok_or_else(|| format!("{verb}: request body must be an object"))?;
    if matches!(verb, "pin" | "unpin" | "delete") {
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("{verb}: missing `id`"))?;
        Ok((format!("entry:{id}"), Some(id.to_string())))
    } else {
        Ok(("all-unpinned".to_string(), None))
    }
}

/// Gate a clipboard mutation before it can load or write the shared history.
/// The read-only `list` verb intentionally remains unauthenticated.
fn build_authorized_reply(svc: &ClipboardService, verb: &str, req_body: Option<&str>) -> String {
    if !matches!(verb, "pin" | "unpin" | "delete" | "clear") {
        return build_reply(svc, verb, req_body);
    }
    let (target, handler_body) = match mutation_request(verb, req_body) {
        Ok(prepared) => prepared,
        Err(error) => return json!({ "error": error }).to_string(),
    };
    let auth_verb = format!("clipboard-{verb}");
    let context = MutationContext {
        verb: &auth_verb,
        node: "clipboard",
        target: &target,
    };
    if let Err(error) = svc
        .authorizer
        .authorize(req_body.expect("mutation request has a body"), context)
    {
        tracing::warn!(target: "mackesd::clipboard", verb, %error, "refused unauthorized clipboard mutation");
        return json!({ "error": format!("{verb}: authorization refused: {error}") }).to_string();
    }
    build_reply(svc, verb, handler_body.as_deref())
}

/// Run the clipboard Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &ClipboardService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &ClipboardService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "clipboard responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_authorized_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "clipboard responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use crate::workers::clipboard_sync::{apply_clip, clip_id};

    const AUTH_KEY: &[u8] = b"clipboard-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn svc() -> (tempfile::TempDir, ClipboardService) {
        let tmp = tempfile::tempdir().unwrap();
        let svc = ClipboardService::new(tmp.path().to_path_buf());
        (tmp, svc)
    }

    fn seed(svc: &ClipboardService, texts: &[&str]) {
        let path = history_path(&svc.workgroup_root);
        let mut h = History::default();
        for t in texts {
            apply_clip(&mut h, t, "n", "t");
        }
        write_history(&path, &h).unwrap();
    }

    #[test]
    fn verbs_and_topic_lock() {
        assert_eq!(action_topic("pin"), "action/clipboard/pin");
        assert!(ACTION_VERBS.contains(&"clear"));
        assert_eq!(ACTION_VERBS.len(), 5);
    }

    #[test]
    fn list_returns_the_shared_history() {
        let (_t, s) = svc();
        seed(&s, &["a", "b"]);
        let r = build_reply(&s, "list", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["entries"].as_array().unwrap().len(), 2);
        // Newest-first: "b" is the front.
        assert_eq!(v["entries"][0]["text"], "b");
    }

    #[test]
    fn pin_then_unpin_round_trips_and_persists() {
        let (_t, s) = svc();
        seed(&s, &["keep"]);
        let id = clip_id("keep");
        let r = build_reply(&s, "pin", Some(&id));
        assert!(r.contains("\"changed\":true"), "{r}");
        let path = history_path(&s.workgroup_root);
        assert!(read_history(&path).entries[0].pinned, "pin persisted");
        // Unpin.
        let r = build_reply(&s, "unpin", Some(&id));
        assert!(r.contains("\"changed\":true"), "{r}");
        assert!(!read_history(&path).entries[0].pinned, "unpin persisted");
    }

    #[test]
    fn pin_unknown_id_is_idempotent_ok_not_error() {
        let (_t, s) = svc();
        seed(&s, &["a"]);
        let r = build_reply(&s, "pin", Some("deadbeef"));
        assert!(r.contains("\"ok\":true"), "{r}");
        assert!(r.contains("\"changed\":false"), "{r}");
    }

    #[test]
    fn pin_missing_body_errors() {
        let (_t, s) = svc();
        assert!(build_reply(&s, "pin", None).contains("error"));
        assert!(build_reply(&s, "pin", Some("  ")).contains("error"));
    }

    #[test]
    fn delete_drops_one_entry_mesh_wide() {
        let (_t, s) = svc();
        seed(&s, &["a", "b", "c"]);
        let r = build_reply(&s, "delete", Some(&clip_id("b")));
        assert!(r.contains("\"changed\":true"), "{r}");
        let h = read_history(&history_path(&s.workgroup_root));
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "a"]
        );
    }

    #[test]
    fn clear_drops_unpinned_keeps_pinned() {
        let (_t, s) = svc();
        seed(&s, &["u1", "keep", "u2"]);
        // Pin "keep".
        let _ = build_reply(&s, "pin", Some(&clip_id("keep")));
        let r = build_reply(&s, "clear", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["cleared"], 2, "two unpinned dropped");
        let h = read_history(&history_path(&s.workgroup_root));
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].text, "keep");
        assert!(h.entries[0].pinned);
    }

    #[test]
    fn set_pin_unpin_re_subjects_to_cap() {
        // 51 unpinned can't coexist; pinning then unpinning past the cap trims.
        let mut h = History::default();
        for i in 0..HISTORY_CAP {
            apply_clip(&mut h, &format!("c{i}"), "n", "t");
        }
        // Pin the oldest, then push one more so we have 51 entries (50 unpinned
        // visible + the pinned one is exempt).
        let oldest = clip_id("c0");
        assert!(set_pin(&mut h, &oldest, true));
        apply_clip(&mut h, "newest", "n", "t");
        // Now 51 entries: 1 pinned + 50 unpinned. Unpin the pinned one →
        // 51 unpinned → trim back to 50.
        assert!(set_pin(&mut h, &oldest, false));
        let unpinned = h.entries.iter().filter(|e| !e.pinned).count();
        assert_eq!(unpinned, HISTORY_CAP);
    }

    #[test]
    fn unknown_verb_errors() {
        let (_t, s) = svc();
        assert!(build_reply(&s, "bogus", None).contains("unknown clipboard verb"));
    }

    fn authorized_service(root: &std::path::Path) -> ClipboardService {
        ClipboardService::new(root.to_path_buf()).with_authorizer(Arc::new(
            ActionAuthorizer::for_test(AUTH_KEY, root.join("auth"), AUTH_NOW),
        ))
    }

    fn authorized_body(verb: &str, id: Option<&str>, nonce: &str) -> String {
        let target = id.map_or_else(|| "all-unpinned".to_string(), |id| format!("entry:{id}"));
        let auth_verb = format!("clipboard-{verb}");
        let unsigned = match id {
            Some(id) => format!(r#"{{"id":"{id}","schema_version":1}}"#),
            None => r#"{"schema_version":1}"#.to_string(),
        };
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: &auth_verb,
                node: "clipboard",
                target: &target,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    #[test]
    fn unsigned_clipboard_mutations_are_refused_before_history_io() {
        let (tmp, s) = svc();
        seed(&s, &["keep"]);
        let id = clip_id("keep");
        let gated = authorized_service(tmp.path());
        let reply = build_authorized_reply(
            &gated,
            "delete",
            Some(&format!(r#"{{"id":"{id}","schema_version":1}}"#)),
        );
        assert!(reply.contains("authorization refused"), "{reply}");
        assert_eq!(read_history(&history_path(tmp.path())).entries.len(), 1);
    }

    #[test]
    fn authorized_mutation_is_single_use_and_exact_body_bound() {
        let (tmp, s) = svc();
        seed(&s, &["drop"]);
        let id = clip_id("drop");
        let gated = authorized_service(tmp.path());
        let body = authorized_body("delete", Some(&id), "clipboard-delete-once");
        let first = build_authorized_reply(&gated, "delete", Some(&body));
        assert!(first.contains("\"changed\":true"), "{first}");
        assert!(read_history(&history_path(tmp.path())).entries.is_empty());
        let replay = build_authorized_reply(&gated, "delete", Some(&body));
        assert!(replay.contains("already used"), "{replay}");
        let tampered = body.replace(&id, "other-entry");
        assert!(build_authorized_reply(&gated, "delete", Some(&tampered))
            .contains("authorization refused"));
    }

    /// CLIP-VIEW-1 producer↔consumer contract lock. The Hub's
    /// `notify_clipboard::parse_list_reply` (a separate crate that never links
    /// mackesd) decodes EXACTLY this `action/clipboard/list` envelope — pin the
    /// field names + shape so a producer-side rename can't silently empty the
    /// Clipboard Viewer. Mirror of `notify_clipboard::tests::
    /// parse_list_reply_decodes_entries_newest_first`.
    #[test]
    fn list_reply_shape_is_the_viewer_contract() {
        let (_t, s) = svc();
        // Seed one unpinned then pin it, so the entry carries every field a row
        // renders (id, text, source, time, pinned=true).
        seed(&s, &["contract"]);
        let _ = build_reply(&s, "pin", Some(&clip_id("contract")));

        let reply = build_reply(&s, "list", None);
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();

        // Envelope: `{ "ok": true, "entries": [ … ] }`.
        assert_eq!(v["ok"], true, "envelope carries ok:true");
        let entries = v["entries"]
            .as_array()
            .expect("entries is an array (the field the viewer reads)");
        assert_eq!(entries.len(), 1);

        // Exact entry field names the viewer's `ClipRow` deserializes — any
        // rename here breaks the Hub silently, so lock them by name + type.
        let e = &entries[0];
        let obj = e.as_object().expect("entry is a JSON object");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["id", "pinned", "source", "text", "time"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "the entry shape is the exact CLIP-VIEW-1 contract"
        );
        assert_eq!(e["id"], clip_id("contract"));
        assert!(e["text"].is_string());
        assert!(e["source"].is_string());
        assert!(e["time"].is_string());
        assert_eq!(e["pinned"], true);
    }
}
