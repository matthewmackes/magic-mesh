//! SWAY-4 (v6.0, Q89/Q94) — per-window mark state.
//!
//! Sway has native window marks (`swaymsg mark / unmark`); this worker
//! bridges the mark store to the Mackes Bus so Portal (mark pills),
//! the border tinter, and elevation-shadow workers can subscribe to
//! mark deltas without querying the compositor directly.
//!
//! Ported from HYP-14 (mde-x c913bed1): pure `MarksStore` + Bus
//! action responder kept intact; IPC event source replaced:
//!   HYP-14: `hyprland_rs::EventStream`  (Hyprland window addresses)
//!   SWAY-4: `swayipc_async` Window events (sway con_id i64)
//!
//! ## Two event sources, one store
//!
//! 1. **swayipc EventStream** (`WindowChange::New` / `Close` /
//!    `Focus`): tracks window lifecycle + fires auto-marks from the
//!    compile-time taxonomy + tag-manifest `marks_default`.
//! 2. **Bus action poll** (`action/marks/{add,remove,list,match}`):
//!    mackesd's Bus action-responder. Polls the persist layer for new
//!    action messages + writes results to `reply/<request-ulid>`.
//!    Every add/remove publishes a delta on `event/marks/<con_id>`.
//!
//! State persists to `~/.local/share/mde/marks/<peer>.toml` on a
//! 60 s tick + shutdown, and is replayed on restart so a mackesd
//! bounce keeps marks for live windows.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt as _;
use mde_bus::hooks::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::{Deserialize, Serialize};
use swayipc_async::{Connection, EventType, WindowChange};

use super::auto_mark::taxonomy_for_app_id;
use super::{ShutdownToken, Worker};

/// How often the Bus action topics are polled for new requests.
const ACTION_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Snapshot cadence.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);
/// Backoff after a swayipc connect failure.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);
/// The four `action/marks/<verb>` topics the responder serves.
const ACTION_VERBS: [&str; 4] = ["add", "remove", "list", "match"];

// ── Store ──────────────────────────────────────────────────────────────────

/// Per-window mark store. Keyed by sway `con_id` (stringified i64).
/// `class` + `title` are kept alongside marks for snapshot replay.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarksStore {
    /// con_id → window identity (for snapshot replay matching).
    identity: HashMap<String, WindowIdentity>,
    /// con_id → marks (sorted, deduped).
    marks: HashMap<String, Vec<String>>,
}

/// Class + title a window reported; used to re-match snapshotted
/// marks to a live window after a mackesd restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WindowIdentity {
    /// App-id / WM_CLASS of the window.
    pub class: String,
    /// Window title at the time it was last seen.
    pub title: String,
}

impl MarksStore {
    /// Add `mark` to `con_id` (idempotent). Returns `true` when the
    /// mark set actually changed.
    pub fn add_mark(&mut self, con_id: &str, mark: &str) -> bool {
        if mark.is_empty() {
            return false;
        }
        let set = self.marks.entry(con_id.to_string()).or_default();
        if set.iter().any(|m| m == mark) {
            return false;
        }
        set.push(mark.to_string());
        set.sort();
        true
    }

    /// Remove `mark` from `con_id`. Returns `true` when something
    /// was removed.
    pub fn remove_mark(&mut self, con_id: &str, mark: &str) -> bool {
        let Some(set) = self.marks.get_mut(con_id) else {
            return false;
        };
        let before = set.len();
        set.retain(|m| m != mark);
        let changed = set.len() != before;
        if set.is_empty() {
            self.marks.remove(con_id);
        }
        changed
    }

    /// List marks on `con_id` (empty when none / unknown).
    #[must_use]
    pub fn list_marks(&self, con_id: &str) -> Vec<String> {
        self.marks.get(con_id).cloned().unwrap_or_default()
    }

    /// Every con_id carrying `mark`, sorted for determinism.
    #[must_use]
    pub fn match_marks(&self, mark: &str) -> Vec<String> {
        let mut hits: Vec<String> = self
            .marks
            .iter()
            .filter(|(_, ms)| ms.iter().any(|m| m == mark))
            .map(|(id, _)| id.clone())
            .collect();
        hits.sort();
        hits
    }

    /// Record a window's identity for snapshot replay matching.
    pub fn note_window(&mut self, con_id: &str, class: &str, title: &str) {
        self.identity.insert(
            con_id.to_string(),
            WindowIdentity {
                class: class.to_string(),
                title: title.to_string(),
            },
        );
    }

    /// Forget a closed window — drops identity + marks.
    pub fn drop_window(&mut self, con_id: &str) {
        self.identity.remove(con_id);
        self.marks.remove(con_id);
    }
}

// ── Bus action request / reply shapes ─────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MarkMutateRequest {
    con_id: String,
    mark: String,
}

#[derive(Debug, Deserialize)]
struct MarkListRequest {
    con_id: String,
}

#[derive(Debug, Deserialize)]
struct MarkMatchRequest {
    mark: String,
}

#[derive(Debug, Serialize)]
struct MarkReply {
    ok: bool,
    changed: bool,
    marks: Vec<String>,
    addrs: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MarkDelta<'a> {
    con_id: &'a str,
    op: &'a str,
    mark: &'a str,
    marks: Vec<String>,
}

// ── Snapshot ───────────────────────────────────────────────────────────────

/// GFS-replicated snapshot of the whole mark store.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarksSnapshot {
    #[serde(default)]
    pub windows: BTreeMap<String, SnapshotEntry>,
}

/// One window's snapshotted identity + marks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub class: String,
    pub title: String,
    pub marks: Vec<String>,
}

impl MarksStore {
    #[must_use]
    pub fn to_snapshot(&self) -> MarksSnapshot {
        let mut snap = MarksSnapshot::default();
        for (con_id, marks) in &self.marks {
            if marks.is_empty() {
                continue;
            }
            let id = self.identity.get(con_id).cloned().unwrap_or_default();
            snap.windows.insert(
                con_id.clone(),
                SnapshotEntry {
                    class: id.class,
                    title: id.title,
                    marks: marks.clone(),
                },
            );
        }
        snap
    }

    #[must_use]
    pub fn from_snapshot(snap: &MarksSnapshot) -> Self {
        let mut store = Self::default();
        for (con_id, entry) in &snap.windows {
            store.identity.insert(
                con_id.clone(),
                WindowIdentity {
                    class: entry.class.clone(),
                    title: entry.title.clone(),
                },
            );
            if !entry.marks.is_empty() {
                let mut marks = entry.marks.clone();
                marks.sort();
                marks.dedup();
                store.marks.insert(con_id.clone(), marks);
            }
        }
        store
    }
}

/// `~/.local/share/mde/marks/<peer>.toml` — per-peer so GFS
/// replication doesn't collide between peers.
#[must_use]
pub fn default_snapshot_path() -> Option<PathBuf> {
    let data = dirs::data_dir()?;
    let host = hostname_string()?;
    Some(data.join("mde").join("marks").join(format!("{host}.toml")))
}

fn hostname_string() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

// ── Pure verb dispatch (testable without swayipc or Persist) ──────────────

/// Outcome of dispatching one Bus action verb against the store.
#[derive(Debug, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// JSON reply to write to `reply/<ulid>`.
    pub reply_json: String,
    /// `Some((con_id, op, mark, marks))` when a delta should publish.
    pub delta: Option<(String, String, String, Vec<String>)>,
}

/// Dispatch one action verb (`add` / `remove` / `list` / `match`)
/// against `store`, mutating it for add/remove. `body` is the request
/// JSON. Unknown verbs / malformed bodies produce `ok:false` + no delta.
pub fn dispatch_action(store: &mut MarksStore, verb: &str, body: &str) -> DispatchOutcome {
    let fail = || DispatchOutcome {
        reply_json: serde_json::to_string(&MarkReply {
            ok: false,
            changed: false,
            marks: Vec::new(),
            addrs: Vec::new(),
        })
        .unwrap_or_else(|_| "{\"ok\":false}".to_string()),
        delta: None,
    };

    match verb {
        "add" | "remove" => {
            let Ok(req) = serde_json::from_str::<MarkMutateRequest>(body) else {
                return fail();
            };
            let changed = if verb == "add" {
                store.add_mark(&req.con_id, &req.mark)
            } else {
                store.remove_mark(&req.con_id, &req.mark)
            };
            let reply = MarkReply {
                ok: true,
                changed,
                marks: store.list_marks(&req.con_id),
                addrs: Vec::new(),
            };
            let delta = if changed {
                Some((
                    req.con_id.clone(),
                    verb.to_string(),
                    req.mark.clone(),
                    store.list_marks(&req.con_id),
                ))
            } else {
                None
            };
            DispatchOutcome {
                reply_json: serde_json::to_string(&reply)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string()),
                delta,
            }
        }
        "list" => {
            let Ok(req) = serde_json::from_str::<MarkListRequest>(body) else {
                return fail();
            };
            let reply = MarkReply {
                ok: true,
                changed: false,
                marks: store.list_marks(&req.con_id),
                addrs: Vec::new(),
            };
            DispatchOutcome {
                reply_json: serde_json::to_string(&reply)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string()),
                delta: None,
            }
        }
        "match" => {
            let Ok(req) = serde_json::from_str::<MarkMatchRequest>(body) else {
                return fail();
            };
            let reply = MarkReply {
                ok: true,
                changed: false,
                marks: Vec::new(),
                addrs: store.match_marks(&req.mark),
            };
            DispatchOutcome {
                reply_json: serde_json::to_string(&reply)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string()),
                delta: None,
            }
        }
        _ => fail(),
    }
}

/// Auto-marks to seed for a newly-opened window from its app_id:
/// the taxonomy bucket (editor/web/shell/mail/chat) + every
/// `marks_default` in a tag manifest whose `apps[]` lists the class.
#[must_use]
pub fn auto_marks_for_class(
    class: &str,
    manifests: Option<&[crate::config::TagManifest]>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(tax) = taxonomy_for_app_id(class) {
        out.push(tax.to_string());
    }
    if let Some(ms) = manifests {
        for m in ms.iter().filter(|m| m.apps.iter().any(|a| a == class)) {
            for mark in m
                .marks_default
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if !out.iter().any(|o| o == mark) {
                    out.push(mark.to_string());
                }
            }
        }
    }
    out
}

// ── Worker ─────────────────────────────────────────────────────────────────

pub struct MarksStateWorker;

impl MarksStateWorker {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarksStateWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for MarksStateWorker {
    fn name(&self) -> &'static str {
        "marks_state"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = default_bus_root() else {
            tracing::debug!("marks_state: no bus root; worker idle");
            return Ok(());
        };
        // Persist is Send but not Sync; wrapping in Mutex makes the field
        // Sync so the future produced by this async fn is Send.  The lock
        // is never held across an .await point — only acquired + released
        // inside synchronous tick handlers.
        let persist = match Persist::open(bus_root) {
            Ok(p) => std::sync::Mutex::new(p),
            Err(e) => {
                tracing::debug!(error = %e, "marks_state: persist open failed; worker idle");
                return Ok(());
            }
        };

        let mut store = load_snapshot().unwrap_or_default();
        let mut cursors: HashMap<String, String> = HashMap::new();

        loop {
            if shutdown.is_shutdown() {
                let _ = save_snapshot(&store);
                return Ok(());
            }

            // Open two swayipc connections: one for event subscription,
            // one for command execution (swaymsg mark/unmark).
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "marks_state: event connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut cmd_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "marks_state: cmd connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };

            let mut events = match event_conn.subscribe([EventType::Window]).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e, "marks_state: subscribe failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };

            let mut snap_tick = tokio::time::interval(SNAPSHOT_INTERVAL);
            snap_tick.tick().await;
            let mut poll_tick = tokio::time::interval(ACTION_POLL_INTERVAL);

            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => {
                        let _ = save_snapshot(&store);
                        return Ok(());
                    }
                    next = events.next() => {
                        match next {
                            Some(Ok(swayipc_async::Event::Window(ev))) => {
                                // Compute marks + publish deltas while the
                                // persist lock is held; release before any
                                // .await so std::sync::Mutex is safe.
                                let sway_cmds = {
                                    let p = persist.lock().expect("marks_state persist lock");
                                    handle_window_event_sync(&*p, &mut store, &ev)
                                };
                                for cmd in sway_cmds {
                                    if let Err(e) = cmd_conn.run_command(&cmd).await {
                                        tracing::debug!(%cmd, error = %e, "marks_state: sway mark cmd failed");
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                tracing::debug!(error = %e, "marks_state: event stream error; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("marks_state: event stream ended; reconnecting");
                                break;
                            }
                        }
                    }
                    _ = poll_tick.tick() => {
                        let p = persist.lock().expect("marks_state persist lock");
                        poll_actions(&*p, &mut store, &mut cursors);
                    }
                    _ = snap_tick.tick() => {
                        let _ = save_snapshot(&store);
                    }
                }
            }
            sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
        }
    }
}

/// Compute auto-marks for a window event and publish Bus deltas.
/// Returns the `swaymsg` command strings to apply (caller runs them async).
/// Lock-and-release pattern: caller holds the Mutex guard for the duration
/// of this sync function; the guard must be dropped before any `.await`.
fn handle_window_event_sync(
    persist: &Persist,
    store: &mut MarksStore,
    ev: &swayipc_async::WindowEvent,
) -> Vec<String> {
    let con_id = ev.container.id.to_string();
    let class = ev
        .container
        .app_id
        .as_deref()
        .or_else(|| {
            ev.container
                .window_properties
                .as_ref()
                .and_then(|p| p.class.as_deref())
        })
        .unwrap_or("");
    let title = ev.container.name.as_deref().unwrap_or("");

    match ev.change {
        WindowChange::New => {
            store.note_window(&con_id, class, title);
            let manifests = crate::config::default_manifests_dir()
                .and_then(|d| crate::config::load_tag_manifests(&d).ok());
            let new_marks: Vec<String> = auto_marks_for_class(class, manifests.as_deref())
                .into_iter()
                .filter(|mark| store.add_mark(&con_id, mark))
                .collect();
            let mut cmds = Vec::with_capacity(new_marks.len());
            for mark in &new_marks {
                publish_delta(persist, &con_id, "add", mark, store.list_marks(&con_id));
                cmds.push(format!("[con_id={con_id}] mark --add {mark}"));
            }
            cmds
        }
        WindowChange::Close => {
            store.drop_window(&con_id);
            Vec::new()
        }
        WindowChange::Focus => {
            store.note_window(&con_id, class, title);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn poll_actions(persist: &Persist, store: &mut MarksStore, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = format!("action/marks/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(%topic, error = %e, "marks_state: action poll failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let outcome = dispatch_action(store, verb, body);
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&outcome.reply_json),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "marks_state: reply write failed");
            }
            if let Some((con_id, op, mark, marks)) = outcome.delta {
                publish_delta(persist, &con_id, &op, &mark, marks);
            }
        }
    }
}

fn publish_delta(persist: &Persist, con_id: &str, op: &str, mark: &str, marks: Vec<String>) {
    let delta = MarkDelta {
        con_id,
        op,
        mark,
        marks,
    };
    let Ok(body) = serde_json::to_string(&delta) else {
        return;
    };
    let topic = format!("event/marks/{con_id}");
    if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
        tracing::debug!(%topic, error = %e, "marks_state: delta publish failed");
    }
}

fn load_snapshot() -> Option<MarksStore> {
    let path = default_snapshot_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let snap: MarksSnapshot = toml::from_str(&raw).ok()?;
    Some(MarksStore::from_snapshot(&snap))
}

fn save_snapshot(store: &MarksStore) -> std::io::Result<()> {
    let Some(path) = default_snapshot_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let snap = store.to_snapshot();
    let body = toml::to_string(&snap)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let mut tmp = path.clone();
    tmp.set_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

async fn sleep_or_shutdown(dur: Duration, shutdown: &mut ShutdownToken) {
    tokio::select! {
        () = shutdown.wait() => {}
        () = tokio::time::sleep(dur) => {}
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_mark_is_idempotent_and_sorted() {
        let mut s = MarksStore::default();
        assert!(s.add_mark("1", "web"));
        assert!(!s.add_mark("1", "web"));
        assert!(s.add_mark("1", "dev"));
        assert_eq!(s.list_marks("1"), vec!["dev", "web"]);
    }

    #[test]
    fn remove_mark_reports_change_and_prunes_empty() {
        let mut s = MarksStore::default();
        s.add_mark("1", "web");
        assert!(s.remove_mark("1", "web"));
        assert!(!s.remove_mark("1", "web"));
        assert!(s.list_marks("1").is_empty());
        assert!(!s.remove_mark("9999", "web"));
    }

    #[test]
    fn match_marks_finds_every_addr_sorted() {
        let mut s = MarksStore::default();
        s.add_mark("2", "web");
        s.add_mark("1", "web");
        s.add_mark("3", "dev");
        assert_eq!(s.match_marks("web"), vec!["1", "2"]);
        assert_eq!(s.match_marks("dev"), vec!["3"]);
        assert!(s.match_marks("none").is_empty());
    }

    #[test]
    fn drop_window_clears_marks_and_identity() {
        let mut s = MarksStore::default();
        s.note_window("1", "firefox", "Mozilla");
        s.add_mark("1", "web");
        s.drop_window("1");
        assert!(s.list_marks("1").is_empty());
        assert!(s.match_marks("web").is_empty());
    }

    #[test]
    fn snapshot_round_trip_preserves_marks_and_identity() {
        let mut s = MarksStore::default();
        s.note_window("1", "firefox", "Mozilla Firefox");
        s.add_mark("1", "web");
        s.add_mark("1", "priority");
        let snap = s.to_snapshot();
        let restored = MarksStore::from_snapshot(&snap);
        assert_eq!(restored.list_marks("1"), vec!["priority", "web"]);
        assert_eq!(restored.to_snapshot(), snap);
    }

    #[test]
    fn snapshot_skips_windows_without_marks() {
        let mut s = MarksStore::default();
        s.note_window("1", "foot", "shell");
        assert!(s.to_snapshot().windows.is_empty());
    }

    #[test]
    fn dispatch_add_mutates_store_and_returns_delta() {
        let mut s = MarksStore::default();
        let out = dispatch_action(&mut s, "add", r#"{"con_id":"1","mark":"web"}"#);
        assert!(out.reply_json.contains("\"ok\":true"));
        assert!(out.reply_json.contains("\"changed\":true"));
        assert!(out.delta.is_some());
        let (con_id, op, mark, _marks) = out.delta.unwrap();
        assert_eq!(con_id, "1");
        assert_eq!(op, "add");
        assert_eq!(mark, "web");
    }

    #[test]
    fn dispatch_add_duplicate_is_noop() {
        let mut s = MarksStore::default();
        dispatch_action(&mut s, "add", r#"{"con_id":"1","mark":"web"}"#);
        let out2 = dispatch_action(&mut s, "add", r#"{"con_id":"1","mark":"web"}"#);
        assert!(out2.reply_json.contains("\"changed\":false"));
        assert!(out2.delta.is_none());
    }

    #[test]
    fn dispatch_remove_missing_mark_is_noop() {
        let mut s = MarksStore::default();
        let out = dispatch_action(&mut s, "remove", r#"{"con_id":"1","mark":"web"}"#);
        assert!(out.reply_json.contains("\"changed\":false"));
        assert!(out.delta.is_none());
    }

    #[test]
    fn dispatch_list_returns_marks_array() {
        let mut s = MarksStore::default();
        s.add_mark("1", "web");
        s.add_mark("1", "dev");
        let out = dispatch_action(&mut s, "list", r#"{"con_id":"1"}"#);
        assert!(out.reply_json.contains("\"ok\":true"));
        assert!(out.reply_json.contains("web"));
        assert!(out.delta.is_none());
    }

    #[test]
    fn dispatch_match_returns_addrs_array() {
        let mut s = MarksStore::default();
        s.add_mark("1", "web");
        s.add_mark("2", "web");
        let out = dispatch_action(&mut s, "match", r#"{"mark":"web"}"#);
        assert!(out.reply_json.contains("\"ok\":true"));
        assert!(out.reply_json.contains("\"1\""));
        assert!(out.delta.is_none());
    }

    #[test]
    fn dispatch_unknown_verb_returns_ok_false() {
        let mut s = MarksStore::default();
        let out = dispatch_action(&mut s, "bogus", "{}");
        assert!(out.reply_json.contains("\"ok\":false"));
    }

    #[test]
    fn dispatch_malformed_body_returns_ok_false() {
        let mut s = MarksStore::default();
        let out = dispatch_action(&mut s, "add", "not json");
        assert!(out.reply_json.contains("\"ok\":false"));
    }

    #[test]
    fn auto_marks_combines_taxonomy_and_manifest() {
        use crate::config::TagManifest;
        let manifests = vec![TagManifest {
            name: "voip".into(),
            apps: vec!["firefox".into()],
            marks_default: "priority,call".into(),
            ..TagManifest::default()
        }];
        let marks = auto_marks_for_class("firefox", Some(&manifests));
        assert!(marks.contains(&"web".to_string()));
        assert!(marks.contains(&"priority".to_string()));
        assert!(marks.contains(&"call".to_string()));
    }

    #[test]
    fn name_is_stable() {
        assert_eq!(MarksStateWorker::new().name(), "marks_state");
    }
}
