//! mde-notify — the shared notification model + bus tail for the
//! **MDE-Notification-Hub** (NOTIFY epic; design:
//! `docs/design/mde-notification-hub.md`).
//!
//! Pure-Rust core that both notification surfaces consume — the standalone
//! layer-shell **center** (NOTIFY-3) and the transient **toast** layer
//! (NOTIFY-4):
//!
//!   * [`AlertItem`] + [`Severity`] + [`Source`] — the typed model (NOTIFY-1).
//!   * [`classify_severity`] / [`classify_source`] / [`severity_token`] — the
//!     grouping + color engine (NOTIFY-2): topic → source, `severity`-field
//!     and/or bus `Priority` → severity, severity → an `mde-theme` Carbon
//!     token (no raw hex — §4).
//!   * [`AlertTail`] — tails the live bus alert lanes via
//!     [`mde_bus::persist::Persist::list_since`] with a per-topic cursor,
//!     deduped by ULID, bounded by a retention horizon (NOTIFY-1).
//!
//! No GUI deps live here; the layer-shell binary is a separate bin target so
//! this model stays render-agnostic + unit-testable.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use mde_bus::persist::{Persist, StoredMessage};
use mde_theme::{Palette, Rgba};

/// Alert severity — the color + sort axis. Ordered most-severe first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Needs attention now (red). `crit`/`error`/urgent.
    Critical,
    /// Worth noticing (amber). `warn`/high.
    Warning,
    /// Informational (blue). `info`/default.
    Info,
    /// A good outcome (green). `ok`/`success`.
    Success,
}

/// Where an alert came from — the top-level grouping in the center's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `fleet/sec` — enrolment / CSR / passcode rotation.
    Security,
    /// Online↔Offline peer transitions.
    Presence,
    /// `event/firewall/*` — denied-connection thresholds.
    Firewall,
    /// `compute/event/*` — VM lifecycle (start/stop/crash).
    Compute,
    /// `fdo/*` — a desktop app's freedesktop notification (via bus_bridge).
    DesktopApp,
    /// `peer/<host>/alerts` — a specific mesh node's alert lane.
    Peer(String),
    /// `mackesd::alert` + metrics + anything else mesh-internal.
    System,
}

impl Source {
    /// A stable display label for the group header.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Source::Security => "Security".to_string(),
            Source::Presence => "Presence".to_string(),
            Source::Firewall => "Firewall".to_string(),
            Source::Compute => "Compute".to_string(),
            Source::DesktopApp => "Desktop apps".to_string(),
            Source::Peer(h) => format!("Peer: {h}"),
            Source::System => "System".to_string(),
        }
    }
}

/// One notification, normalized from a bus message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertItem {
    /// Bus ULID — the stable dedup key.
    pub id: String,
    /// Epoch milliseconds the bus recorded it.
    pub ts_unix_ms: i64,
    /// Color/sort axis.
    pub severity: Severity,
    /// Grouping axis.
    pub source: Source,
    /// Raw bus topic (for drill / filter).
    pub topic: String,
    /// Originating mesh node, when known.
    pub host: Option<String>,
    /// Short title (the alert kind / app name).
    pub title: String,
    /// Body / summary text.
    pub body: String,
    /// Whether the operator has acknowledged it.
    pub read: bool,
}

/// Map a `severity`-field string and/or the bus `priority` string to a
/// [`Severity`]. The explicit `severity` field wins; the bus priority is the
/// fallback (NOTIFY-2 — "field AND/OR Priority"). Unknown → `Info`.
#[must_use]
pub fn classify_severity(severity_field: Option<&str>, priority: &str) -> Severity {
    if let Some(s) = severity_field {
        match s.trim().to_ascii_lowercase().as_str() {
            "crit" | "critical" | "error" | "err" | "fatal" => return Severity::Critical,
            "warn" | "warning" => return Severity::Warning,
            "info" | "notice" | "debug" => return Severity::Info,
            "ok" | "success" | "resolved" => return Severity::Success,
            _ => {} // fall through to priority
        }
    }
    match priority.trim().to_ascii_lowercase().as_str() {
        "urgent" => Severity::Critical,
        "high" => Severity::Warning,
        "min" | "low" => Severity::Info,
        _ => Severity::Info, // "default" + anything else
    }
}

/// Map a bus `topic` to its [`Source`] group (NOTIFY-2 — topic-prefix → source).
#[must_use]
pub fn classify_source(topic: &str) -> Source {
    let t = topic.trim();
    if t == "fleet/sec" || t.starts_with("fleet/sec/") {
        Source::Security
    } else if t.starts_with("event/firewall") {
        Source::Firewall
    } else if t.starts_with("compute/event") {
        Source::Compute
    } else if t.contains("presence") {
        Source::Presence
    } else if t.starts_with("fdo/") {
        Source::DesktopApp
    } else if let Some(host) = peer_host(t) {
        Source::Peer(host)
    } else {
        Source::System
    }
}

/// Extract `<host>` from a `peer/<host>/alerts` topic, else `None`.
fn peer_host(topic: &str) -> Option<String> {
    let rest = topic.strip_prefix("peer/")?;
    let (host, tail) = rest.split_once('/')?;
    (tail == "alerts" && !host.is_empty()).then(|| host.to_string())
}

/// `true` when `topic` is one of the alert lanes the Hub tails.
#[must_use]
pub fn topic_is_alert_lane(topic: &str) -> bool {
    let t = topic.trim();
    t == "fleet/sec"
        || t.starts_with("fleet/sec/")
        || t.starts_with("event/firewall")
        || t.starts_with("compute/event")
        || t.starts_with("fdo/")
        || t == "mackesd::alert"
        || t.contains("presence")
        || peer_host(t).is_some()
}

/// The `mde-theme` Carbon token a severity renders in (NOTIFY-2 — no raw hex;
/// the caller supplies the active [`Palette`]).
#[must_use]
pub fn severity_token(severity: Severity, palette: &Palette) -> Rgba {
    match severity {
        Severity::Critical => palette.danger,
        Severity::Warning => palette.warning,
        Severity::Info => palette.accent,
        Severity::Success => palette.success,
    }
}

/// NOTIFY-5 — the freedesktop XDG sound-theme name for a severity, played via
/// `canberra-gtk-play -i <name>` (falling back to `paplay` of the matching
/// `/usr/share/sounds/freedesktop/stereo/<name>.oga`). Using named theme sounds
/// means no bundled audio assets to license/ship (operator decision, 2026-06-15).
#[must_use]
pub fn severity_sound_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "dialog-error",
        Severity::Warning => "dialog-warning",
        Severity::Info => "message",
        Severity::Success => "complete",
    }
}

/// NOTIFY-5 — per-group sound preferences. Persisted as YAML next to the bus
/// data so the toast surface + the (NOTIFY-6) settings sidecar share one file.
/// `enabled` is the global on/off; `muted_sources` carries the [`Source::label`]
/// of any group the operator silenced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SoundSettings {
    /// Master switch — `false` silences every group.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Group labels (per [`Source::label`]) the operator has muted.
    #[serde(default)]
    pub muted_sources: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            muted_sources: Vec::new(),
        }
    }
}

impl SoundSettings {
    /// Load from `<bus_root>/notify-sound.yaml`; missing/corrupt → defaults
    /// (sound on) so a bad file never silently kills audio.
    #[must_use]
    pub fn load(bus_root: &std::path::Path) -> Self {
        let path = bus_root.join("notify-sound.yaml");
        std::fs::read(&path)
            .ok()
            .and_then(|b| serde_yaml::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomic-write to `<bus_root>/notify-sound.yaml` (temp + rename).
    pub fn save(&self, bus_root: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(bus_root)?;
        let s = serde_yaml::to_string(self)
            .map_err(|e| std::io::Error::other(format!("serialize notify-sound.yaml: {e}")))?;
        let tmp = bus_root.join("notify-sound.yaml.tmp");
        let dst = bus_root.join("notify-sound.yaml");
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, &dst)
    }

    /// True when this source group is currently muted.
    #[must_use]
    pub fn is_muted(&self, source: &Source) -> bool {
        self.muted_sources.contains(&source.label())
    }
}

/// NOTIFY-5 — the sound to play for a fresh alert, or `None` when it should be
/// silent. Mirrors the toast's gating: sound is silent under DND (Critical
/// bypasses, matching the emergency rule), when globally disabled, or when the
/// alert's source group is muted. Pure + testable.
#[must_use]
pub fn sound_for_alert(
    settings: &SoundSettings,
    item: &AlertItem,
    dnd_active: bool,
) -> Option<&'static str> {
    if !settings.enabled {
        return None;
    }
    if dnd_active && item.severity != Severity::Critical {
        return None;
    }
    if settings.is_muted(&item.source) {
        return None;
    }
    Some(severity_sound_name(item.severity))
}

/// Normalize one [`StoredMessage`] into an [`AlertItem`]. The body is parsed as
/// JSON for `severity`/`host`/`title`/`summary` when present; everything
/// degrades gracefully (a non-JSON body becomes the alert text).
#[must_use]
pub fn alert_from_message(msg: &StoredMessage) -> AlertItem {
    let body_json: Option<serde_json::Value> = msg
        .body
        .as_deref()
        .and_then(|b| serde_json::from_str(b).ok());
    let field = |k: &str| -> Option<String> {
        body_json
            .as_ref()
            .and_then(|v| v.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let severity = classify_severity(field("severity").as_deref(), &msg.priority);
    let source = classify_source(&msg.topic);
    let host = field("host").or_else(|| match &source {
        Source::Peer(h) => Some(h.clone()),
        _ => None,
    });
    let title = field("alert")
        .or_else(|| field("title"))
        .or_else(|| field("appName"))
        .unwrap_or_else(|| msg.topic.clone());
    let body = field("summary")
        .or_else(|| field("message"))
        .or_else(|| field("body"))
        .or_else(|| msg.body.clone())
        .unwrap_or_default();
    AlertItem {
        id: msg.ulid.clone(),
        ts_unix_ms: msg.ts_unix_ms,
        severity,
        source,
        topic: msg.topic.clone(),
        host,
        title,
        body,
        read: false,
    }
}

/// Default cap on remembered ULIDs (the dedup horizon). A long uptime can't
/// grow the seen-set unbounded; the oldest IDs age out FIFO.
pub const DEFAULT_RETENTION: usize = 2000;

/// Stateful tail over the bus alert lanes. Construct once, then [`poll`] on a
/// cadence; each call returns only the *new* alerts since the last poll.
///
/// [`poll`]: AlertTail::poll
#[derive(Debug)]
pub struct AlertTail {
    /// topic → last-seen ULID (the `list_since` cursor).
    cursors: HashMap<String, String>,
    /// Dedup set (also guards a topic re-listing from re-emitting).
    seen: HashSet<String>,
    /// FIFO of seen IDs to bound `seen`.
    seen_order: Vec<String>,
    /// Max remembered IDs.
    retention: usize,
}

impl Default for AlertTail {
    fn default() -> Self {
        Self::new(DEFAULT_RETENTION)
    }
}

impl AlertTail {
    /// A tail remembering up to `retention` ULIDs for dedup.
    #[must_use]
    pub fn new(retention: usize) -> Self {
        Self {
            cursors: HashMap::new(),
            seen: HashSet::new(),
            seen_order: Vec::new(),
            retention: retention.max(1),
        }
    }

    /// Poll the bus: enumerate alert-lane topics, read each since its cursor,
    /// and return the new, deduped [`AlertItem`]s (oldest first). Idempotent —
    /// a second poll with no new bus traffic returns empty.
    pub fn poll(&mut self, persist: &Persist) -> Vec<AlertItem> {
        let topics = persist.list_topics().unwrap_or_default();
        let mut fresh = Vec::new();
        for topic in topics.into_iter().filter(|t| topic_is_alert_lane(t)) {
            let cursor = self.cursors.get(&topic).cloned();
            let msgs = match persist.list_since(&topic, cursor.as_deref()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            for msg in msgs {
                self.cursors.insert(topic.clone(), msg.ulid.clone());
                if self.mark_seen(&msg.ulid) {
                    fresh.push(alert_from_message(&msg));
                }
            }
        }
        fresh.sort_by_key(|a| a.ts_unix_ms);
        fresh
    }

    /// NOTIFY-DIST-2 — poll the replicated shared-alerts directory (every peer's
    /// mirrored alerts under `<workgroup_root>/.mesh-alerts/<host>/`) and return
    /// new, deduped [`AlertItem`]s (oldest first). Shares the dedup set with
    /// [`poll`](Self::poll), so a node's own alert (present in both its local bus
    /// and its own mirror) is shown once. This is what makes the panel mesh-wide:
    /// each node only ever tails its *local* bus, but reads *everyone's* mirror.
    pub fn poll_shared(&mut self, workgroup_root: &std::path::Path) -> Vec<AlertItem> {
        self.dedup_fresh(read_shared_alert_items(workgroup_root))
    }

    /// Dedup a batch of candidate items against the seen-set + sort oldest-first.
    /// Split out so the (FUSE-backed, potentially-wedged) directory read can run
    /// on a helper thread via [`read_shared_alert_items`] while the dedup — which
    /// needs `&mut self` — stays on the caller's thread (NOTIFY-UI-4: the read
    /// must never block the UI loop, or the Action Center never maps its window).
    pub fn dedup_fresh(&mut self, items: Vec<AlertItem>) -> Vec<AlertItem> {
        let mut fresh: Vec<AlertItem> = items
            .into_iter()
            .filter(|it| self.mark_seen(&it.id))
            .collect();
        fresh.sort_by_key(|a| a.ts_unix_ms);
        fresh
    }

    /// Record `id` as seen; returns `true` if it's new. Bounds the set FIFO.
    fn mark_seen(&mut self, id: &str) -> bool {
        if self.seen.contains(id) {
            return false;
        }
        self.seen.insert(id.to_string());
        self.seen_order.push(id.to_string());
        if self.seen_order.len() > self.retention {
            let old = self.seen_order.remove(0);
            self.seen.remove(&old);
        }
        true
    }
}

/// NOTIFY-DIST-2 — the replicated shared-alerts directory on the workgroup
/// (QNM-Shared) root. Each node mirrors its own alert-lane messages under
/// `<root>/.mesh-alerts/<host>/` so every node's panel can render mesh-wide.
#[must_use]
pub fn shared_alert_dir(workgroup_root: &std::path::Path) -> std::path::PathBuf {
    workgroup_root.join(".mesh-alerts")
}

/// Read + parse every mirrored shared-alert message into an [`AlertItem`].
///
/// Pure and `&self`-free (no dedup state), so it is safe to run on a helper
/// thread. The shared dir lives on the QNM-Shared (Syncthing-replicated) share; a wedged
/// share makes these reads block uninterruptibly, so the caller runs this off the
/// UI thread and picks the result up non-blockingly (NOTIFY-UI-4). Dedup the
/// result with [`AlertTail::dedup_fresh`].
#[must_use]
pub fn read_shared_alert_items(workgroup_root: &std::path::Path) -> Vec<AlertItem> {
    let base = shared_alert_dir(workgroup_root);
    let mut items = Vec::new();
    let Ok(hosts) = std::fs::read_dir(&base) else {
        return items;
    };
    for host in hosts.flatten() {
        if !host.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(host.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            let Ok(msg) = serde_json::from_slice::<StoredMessage>(&bytes) else {
                continue;
            };
            items.push(alert_from_message(&msg));
        }
    }
    items
}

/// NOTIFY-DIST-2 — mirror one alert message into the shared dir under `host`,
/// atomically (temp + rename). Idempotent: an existing `<ulid>.json` is left
/// untouched (the mirror worker re-runs over the same lane). Used by the
/// mackesd alert-mirror worker; the receiving side is [`AlertTail::poll_shared`].
///
/// # Errors
/// Filesystem create/write/rename failures (e.g. the mount is read-only).
pub fn write_shared_alert(
    workgroup_root: &std::path::Path,
    host: &str,
    msg: &StoredMessage,
) -> std::io::Result<()> {
    let dir = shared_alert_dir(workgroup_root).join(host);
    std::fs::create_dir_all(&dir)?;
    let dst = dir.join(format!("{}.json", msg.ulid));
    if dst.exists() {
        return Ok(());
    }
    let body = serde_json::to_string(msg)
        .map_err(|e| std::io::Error::other(format!("serialize alert: {e}")))?;
    let tmp = dir.join(format!("{}.json.tmp", msg.ulid));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(ulid: &str, topic: &str, priority: &str, body: &str) -> StoredMessage {
        StoredMessage {
            ulid: ulid.to_string(),
            topic: topic.to_string(),
            priority: priority.to_string(),
            title: None,
            body: Some(body.to_string()),
            ts_unix_ms: 1,
            file_path: String::new(),
            actions: Vec::new(),
            reply_to: None,
        }
    }

    #[test]
    fn shared_alert_roundtrip_and_dedup() {
        // NOTIFY-DIST-2 — mirror two peers' alerts to the shared dir, then a
        // node's tail picks them all up via poll_shared; a second poll is empty.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = msg("01AAA", "peer/alpha/alerts", "high", "{\"summary\":\"a\"}");
        let b = msg("01BBB", "fleet/sec", "urgent", "{\"summary\":\"b\"}");
        write_shared_alert(root, "alpha", &a).unwrap();
        write_shared_alert(root, "bravo", &b).unwrap();
        // Idempotent re-mirror is a no-op (no error, no dup file content).
        write_shared_alert(root, "alpha", &a).unwrap();

        let mut tail = AlertTail::default();
        let got = tail.poll_shared(root);
        let ids: Vec<&str> = got.iter().map(|i| i.id.as_str()).collect();
        assert!(
            ids.contains(&"01AAA") && ids.contains(&"01BBB"),
            "got {ids:?}"
        );
        assert_eq!(got.len(), 2);
        // Dedup: nothing new on the next poll.
        assert!(tail.poll_shared(root).is_empty());
    }

    #[test]
    fn poll_shared_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tail = AlertTail::default();
        assert!(tail.poll_shared(tmp.path()).is_empty());
    }

    #[test]
    fn read_shared_alert_items_is_pure_and_dedup_is_separate() {
        // NOTIFY-UI-4 — the FUSE-backed read (`read_shared_alert_items`) is a pure
        // function with no dedup state, so it can run on a helper thread; the
        // dedup (`dedup_fresh`) happens separately on the caller's thread.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = msg("01AAA", "peer/alpha/alerts", "high", "{\"summary\":\"a\"}");
        let b = msg("01BBB", "fleet/sec", "urgent", "{\"summary\":\"b\"}");
        write_shared_alert(root, "alpha", &a).unwrap();
        write_shared_alert(root, "bravo", &b).unwrap();

        // The pure read returns every item every time (no dedup of its own).
        assert_eq!(read_shared_alert_items(root).len(), 2);
        assert_eq!(read_shared_alert_items(root).len(), 2);

        // dedup_fresh applied to the same batch yields each once, then nothing.
        let mut tail = AlertTail::default();
        let first = tail.dedup_fresh(read_shared_alert_items(root));
        assert_eq!(first.len(), 2);
        assert!(tail.dedup_fresh(read_shared_alert_items(root)).is_empty());
    }

    #[test]
    fn severity_from_field_takes_precedence() {
        // Field wins over priority.
        assert_eq!(
            classify_severity(Some("crit"), "default"),
            Severity::Critical
        );
        assert_eq!(classify_severity(Some("warn"), "urgent"), Severity::Warning);
        assert_eq!(classify_severity(Some("ok"), "urgent"), Severity::Success);
    }

    #[test]
    fn severity_falls_back_to_priority() {
        assert_eq!(classify_severity(None, "urgent"), Severity::Critical);
        assert_eq!(classify_severity(None, "high"), Severity::Warning);
        assert_eq!(classify_severity(None, "default"), Severity::Info);
        assert_eq!(classify_severity(None, "min"), Severity::Info);
        // Unknown field with no priority match → Info.
        assert_eq!(classify_severity(Some("weird"), "default"), Severity::Info);
    }

    #[test]
    fn source_maps_each_lane() {
        assert_eq!(classify_source("fleet/sec"), Source::Security);
        assert_eq!(classify_source("event/firewall/host-a"), Source::Firewall);
        assert_eq!(classify_source("compute/event/node2"), Source::Compute);
        assert_eq!(classify_source("fdo/firefox"), Source::DesktopApp);
        assert_eq!(
            classify_source("peer/UNIT-EAGLE/alerts"),
            Source::Peer("UNIT-EAGLE".to_string())
        );
        assert_eq!(classify_source("peer/x/presence"), Source::Presence);
        assert_eq!(classify_source("mackesd::alert"), Source::System);
    }

    #[test]
    fn alert_lane_predicate_matches_the_design_lanes() {
        for t in [
            "fleet/sec",
            "event/firewall/h",
            "compute/event/n",
            "fdo/app",
            "mackesd::alert",
            "peer/h/alerts",
            "peer/h/presence",
        ] {
            assert!(topic_is_alert_lane(t), "should tail {t}");
        }
        for t in [
            "action/connect/devices",
            "mesh/directory",
            "peer/h/heartbeat",
        ] {
            assert!(!topic_is_alert_lane(t), "should NOT tail {t}");
        }
    }

    #[test]
    fn severity_token_maps_to_the_carbon_status_colors() {
        let p = Palette::dark();
        assert_eq!(severity_token(Severity::Critical, &p), p.danger);
        assert_eq!(severity_token(Severity::Warning, &p), p.warning);
        assert_eq!(severity_token(Severity::Info, &p), p.accent);
        assert_eq!(severity_token(Severity::Success, &p), p.success);
    }

    #[test]
    fn alert_from_message_parses_fields_and_classifies() {
        let m = msg(
            "01HID",
            "peer/UNIT-EAGLE/alerts",
            "high",
            r#"{"severity":"warn","host":"UNIT-EAGLE","alert":"mesh.presence.offline","summary":"node went offline"}"#,
        );
        let a = alert_from_message(&m);
        assert_eq!(a.id, "01HID");
        assert_eq!(a.severity, Severity::Warning);
        assert_eq!(a.source, Source::Peer("UNIT-EAGLE".to_string()));
        assert_eq!(a.host.as_deref(), Some("UNIT-EAGLE"));
        assert_eq!(a.title, "mesh.presence.offline");
        assert_eq!(a.body, "node went offline");
        assert!(!a.read);
    }

    #[test]
    fn alert_from_message_degrades_for_non_json_body() {
        let m = msg("01X", "fdo/firefox", "default", "Download complete");
        let a = alert_from_message(&m);
        assert_eq!(a.source, Source::DesktopApp);
        assert_eq!(a.severity, Severity::Info);
        assert_eq!(a.body, "Download complete");
        assert_eq!(a.title, "fdo/firefox");
    }

    #[test]
    fn tail_dedups_by_ulid_and_bounds_the_seen_set() {
        let mut tail = AlertTail::new(2);
        assert!(tail.mark_seen("a"));
        assert!(tail.mark_seen("b"));
        assert!(!tail.mark_seen("a"), "already seen");
        // Retention 2: adding c evicts a (FIFO), so a is 'new' again.
        assert!(tail.mark_seen("c"));
        assert!(tail.mark_seen("a"), "a aged out of the seen horizon");
    }

    #[test]
    fn tail_poll_reads_alert_lanes_dedups_and_advances_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        // One alert-lane message + one non-alert message.
        persist
            .write(
                "peer/h1/alerts",
                mde_bus::hooks::config::Priority::High,
                None,
                Some(r#"{"severity":"warn","summary":"disk low"}"#),
            )
            .unwrap();
        persist
            .write(
                "action/connect/devices",
                mde_bus::hooks::config::Priority::Default,
                None,
                Some("[]"),
            )
            .unwrap();

        let mut tail = AlertTail::default();
        let first = tail.poll(&persist);
        assert_eq!(first.len(), 1, "only the alert-lane message surfaces");
        assert_eq!(first[0].source, Source::Peer("h1".to_string()));
        assert_eq!(first[0].severity, Severity::Warning);
        // Second poll, no new traffic → empty (cursor advanced + deduped).
        assert!(tail.poll(&persist).is_empty());
    }

    // ── NOTIFY-5: sound layer ───────────────────────────────────────────

    fn alert(src: Source, sev: Severity) -> AlertItem {
        AlertItem {
            id: "x".into(),
            ts_unix_ms: 0,
            severity: sev,
            source: src,
            topic: "t".into(),
            host: None,
            title: "t".into(),
            body: String::new(),
            read: false,
        }
    }

    #[test]
    fn severity_sound_names_are_freedesktop() {
        assert_eq!(severity_sound_name(Severity::Critical), "dialog-error");
        assert_eq!(severity_sound_name(Severity::Warning), "dialog-warning");
        assert_eq!(severity_sound_name(Severity::Info), "message");
        assert_eq!(severity_sound_name(Severity::Success), "complete");
    }

    #[test]
    fn sound_default_on_plays_for_ordinary_alert() {
        let s = SoundSettings::default();
        assert!(s.enabled);
        assert_eq!(
            sound_for_alert(&s, &alert(Source::Security, Severity::Warning), false),
            Some("dialog-warning")
        );
    }

    #[test]
    fn sound_global_disable_silences_all() {
        let s = SoundSettings {
            enabled: false,
            muted_sources: Vec::new(),
        };
        assert_eq!(
            sound_for_alert(&s, &alert(Source::Security, Severity::Critical), false),
            None
        );
    }

    #[test]
    fn sound_muted_group_is_silent_others_play() {
        let s = SoundSettings {
            enabled: true,
            muted_sources: vec![Source::Firewall.label()],
        };
        assert_eq!(
            sound_for_alert(&s, &alert(Source::Firewall, Severity::Warning), false),
            None
        );
        assert!(sound_for_alert(&s, &alert(Source::Security, Severity::Warning), false).is_some());
    }

    #[test]
    fn sound_dnd_silences_ordinary_but_not_critical() {
        let s = SoundSettings::default();
        assert_eq!(
            sound_for_alert(&s, &alert(Source::System, Severity::Info), true),
            None
        );
        assert_eq!(
            sound_for_alert(&s, &alert(Source::Security, Severity::Critical), true),
            Some("dialog-error")
        );
    }

    #[test]
    fn sound_settings_round_trip_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let original = SoundSettings {
            enabled: false,
            muted_sources: vec!["Compute".into(), "Presence".into()],
        };
        original.save(dir.path()).unwrap();
        let loaded = SoundSettings::load(dir.path());
        assert_eq!(loaded, original);
    }

    #[test]
    fn sound_settings_missing_file_defaults_on() {
        let dir = tempfile::tempdir().unwrap();
        // No file → defaults (sound on) so a fresh install isn't silent.
        assert_eq!(SoundSettings::load(dir.path()), SoundSettings::default());
    }
}
