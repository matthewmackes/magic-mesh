//! BUS-7.2 — Network → Mackes Bus panel.
//!
//! 5-tab panel: Topics / Subscriptions / Hooks / Audit / DND.
//! BUS-7.2 ships the skeleton + DND tab real content (BUS-7.6).
//! BUS-7.3..BUS-7.5 fill in the remaining content tabs.
//!
//! Cite: docs/design/v6.x-mackes-bus.md §7 (operator surfaces);
//! ref: Linear (notification-settings tab bar).

use std::path::PathBuf;

use cosmic::iced::widget::button::Status as ButtonStatus;
use cosmic::iced::widget::{button, column, row, text, text_editor, text_input, Space};
use cosmic::iced::{alignment, Background, Border, Color, Length, Task};
use cosmic::Theme;
use mde_theme::{CardState, EmptyState, FontSize, Icon, ObjectCard, Palette, Radii, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{empty_state, panel_container};

/// libcosmic-themed element alias: cosmic widgets default their theme param to
/// `cosmic::Theme`, so the panel's `Element<'_, M>` annotations must carry it
/// (matches `cosmic::Element`). See CUT-1 port rule 6.
type Element<'a, M> = cosmic::iced::Element<'a, M, Theme>;

// ─── local mirror types ──────────────────────────────────────────────────────
// These shadow the mde-bus structs so the workbench crate does not
// need the full mde-bus dep. Same serde field names → same YAML/JSON.

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DndStatusJson {
    #[serde(default)]
    active: bool,
    #[serde(default)]
    since_unix_ms: i64,
    #[serde(default)]
    set_by_peer: String,
    #[serde(default)]
    snoozes: Vec<SnoozeJson>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SnoozeJson {
    topic: String,
    until_unix_ms: i64,
    #[serde(default)]
    set_by_peer: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SubsYaml {
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    mute: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quiet_hours: Option<QuietHoursYaml>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QuietHoursYaml {
    start: String,
    end: String,
}

// ─── Topics tab data ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TopicInfo {
    name: String,
    message_count: usize,
    last_activity_ms: i64,
    priority: String,
}

#[derive(Debug, Clone, Default)]
pub struct TopicMessage {
    title: String,
    body_preview: String,
    published_at_ms: i64,
    publisher: String,
}

#[derive(Debug, Clone, Default)]
pub struct TopicsTabState {
    pub list: Vec<TopicInfo>,
    pub expanded: Option<String>,
    pub messages: Vec<TopicMessage>,
    pub loading: bool,
    pub messages_loading: bool,
    pub error: Option<String>,
    pub loaded: bool,
}

// ─── Hook samples ────────────────────────────────────────────────────────────

struct HookSample {
    label: &'static str,
    yaml: &'static str,
}

const HOOK_SAMPLES: &[HookSample] = &[
    HookSample {
        label: "GitHub push",
        yaml: "adapters:\n  github:\n    rules:\n      - name: github-push\n        match:\n          event: push\n        publish:\n          topic: gh/push\n          priority: default\n          title: \"{{ repo }} push to {{ branch }}\"\n          body: \"{{ pusher }} pushed {{ commit_count }} commits\"\n",
    },
    HookSample {
        label: "Gitea push",
        yaml: "adapters:\n  gitea:\n    rules:\n      - name: gitea-push\n        match:\n          event: push\n        publish:\n          topic: git/push\n          priority: default\n          title: \"{{ repo }} push by {{ pusher }}\"\n          body: \"{{ commit_count }} new commits on {{ branch }}\"\n",
    },
    HookSample {
        label: "Home Assistant state",
        yaml: "adapters:\n  home_assistant:\n    rules:\n      - name: ha-state-change\n        match:\n          event: state_changed\n        publish:\n          topic: home/state\n          priority: default\n          title: \"{{ entity_id }} changed\"\n          body: \"New state: {{ new_state }}\"\n",
    },
    HookSample {
        label: "Generic webhook",
        yaml: "adapters:\n  generic:\n    rules:\n      - name: generic-event\n        publish:\n          topic: webhook/events\n          priority: default\n          title: \"Incoming webhook\"\n          body: \"Event received\"\n",
    },
];

// ─── Hooks tab state ──────────────────────────────────────────────────────────

pub struct HooksTabState {
    pub content: text_editor::Content,
    pub validation_error: Option<String>,
    pub loading: bool,
    pub saving: bool,
    pub loaded: bool,
}

impl HooksTabState {
    fn yaml_text(&self) -> String {
        self.content.text()
    }

    fn validate(&mut self) {
        let txt = self.yaml_text();
        self.validation_error = if txt.trim().is_empty() {
            None
        } else {
            match serde_yaml::from_str::<serde_yaml::Value>(&txt) {
                Ok(v) => {
                    if v.as_mapping()
                        .map(|m| m.contains_key("adapters"))
                        .unwrap_or(false)
                    {
                        None
                    } else {
                        Some("Top-level key `adapters` missing.".to_string())
                    }
                }
                Err(e) => Some(e.to_string()),
            }
        };
    }
}

impl Default for HooksTabState {
    fn default() -> Self {
        Self {
            content: text_editor::Content::new(),
            validation_error: None,
            loading: false,
            saving: false,
            loaded: false,
        }
    }
}

impl Clone for HooksTabState {
    fn clone(&self) -> Self {
        Self {
            content: text_editor::Content::with_text(&self.yaml_text()),
            validation_error: self.validation_error.clone(),
            loading: self.loading,
            saving: self.saving,
            loaded: self.loaded,
        }
    }
}

impl std::fmt::Debug for HooksTabState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HooksTabState")
            .field("validation_error", &self.validation_error)
            .field("loading", &self.loading)
            .field("saving", &self.saving)
            .field("loaded", &self.loaded)
            .finish()
    }
}

// ─── Subscriptions tab state ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct SubsTabState {
    /// Subscribed topic patterns (from `subs.yaml topics:`).
    pub topics: Vec<String>,
    /// Muted patterns (from `subs.yaml mute:`).
    pub muted: Vec<String>,
    pub new_topic: String,
    pub peer_input: String,
    pub loading: bool,
    pub error: Option<String>,
    pub loaded: bool,
}

// ─── DND tab state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DndTabState {
    pub status: Option<DndStatusJson>,
    pub quiet_start: String,
    pub quiet_end: String,
    pub loading: bool,
    pub saving: bool,
    pub error: Option<String>,
    pub loaded: bool,
}

// ─── Tab enum ────────────────────────────────────────────────────────────────

/// The five Bus panel tabs, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Topics,
    Subscriptions,
    Hooks,
    Audit,
    Dnd,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Topics => "Topics",
            Self::Subscriptions => "Subscriptions",
            Self::Hooks => "Hooks",
            Self::Audit => "Audit",
            Self::Dnd => "DND",
        }
    }

    const ALL: [Tab; 5] = [
        Tab::Topics,
        Tab::Subscriptions,
        Tab::Hooks,
        Tab::Audit,
        Tab::Dnd,
    ];
}

// ─── Panel struct ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct MeshBusPanel {
    pub active_tab: Tab,
    pub topics: TopicsTabState,
    pub subs: SubsTabState,
    pub hooks: HooksTabState,
    pub dnd: DndTabState,
}

// ─── Messages ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    SelectTab(Tab),
    // Topics tab
    TopicsLoaded(Result<Vec<TopicInfo>, String>),
    TopicSelected(String),
    TopicDeselected,
    TopicMessagesLoaded(Result<Vec<TopicMessage>, String>),
    TopicsRefreshClicked,
    // Hooks tab
    HooksLoaded(Result<String, String>),
    HooksEdited(text_editor::Action),
    HooksSaveClicked,
    HooksSaveDone(Result<(), String>),
    HooksSampleInserted(usize),
    // Subscriptions tab
    SubsLoaded(Result<(Vec<String>, Vec<String>), String>),
    SubsNewTopicChanged(String),
    SubsAddClicked,
    SubsAddDone(Result<(), String>),
    SubsRemoveClicked(String),
    SubsRemoveDone(Result<(), String>),
    SubsPeerInputChanged(String),
    SubsMatchPeerClicked,
    SubsMatchPeerDone(Result<Vec<String>, String>),
    SubsRefreshClicked,
    // DND tab
    DndLoaded(Result<(DndStatusJson, String, String), String>),
    DndToggleClicked,
    DndToggleDone(Result<(), String>),
    DndRefreshClicked,
    DndQuietStartChanged(String),
    DndQuietEndChanged(String),
    DndSaveClicked,
    DndSaveDone(Result<(), String>),
}

// ─── Async helpers ────────────────────────────────────────────────────────────

fn bus_root() -> Option<PathBuf> {
    // Honor MDE_BUS_ROOT (the shared /run/mde-bus spool the daemon + GUIs
    // share) via the canonical resolver — was a per-HOME path that read an
    // empty ~/.local/share/mde/bus while all real traffic landed on
    // /run/mde-bus, so the panel showed no activity.
    mde_bus::client_data_dir()
}

/// Walk `bus_root()` (BFS, depth ≤ 4) and return one
/// [`TopicInfo`] per leaf directory that contains `.json` files.
async fn load_topics() -> Result<Vec<TopicInfo>, String> {
    // Topic dirs live directly under the bus root (action/, event/, mesh/,
    // …), not under a `topics/` subdir — walk the root itself.
    let topics_dir = bus_root().ok_or_else(|| "no data dir".to_string())?;

    if tokio::fs::metadata(&topics_dir).await.is_err() {
        return Ok(Vec::new());
    }

    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = Default::default();
    queue.push_back((topics_dir.clone(), 0));
    let mut leaf_dirs: Vec<(PathBuf, String)> = Vec::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if depth > 4 {
            continue;
        }
        let rel = dir
            .strip_prefix(&topics_dir)
            .unwrap_or(std::path::Path::new(""))
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");

        let mut rd = match tokio::fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut has_json = false;
        let mut subdirs = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                subdirs.push(path);
            } else if path.extension().map(|e| e == "json").unwrap_or(false) {
                has_json = true;
            }
        }
        if has_json && !rel.is_empty() {
            leaf_dirs.push((dir.clone(), rel));
        }
        for sub in subdirs {
            // AUDIT-MESH-9 — don't descend into the request/reply plumbing tree
            // (`reply/<ulid>/…`): it's RPC correlation spool, not a user-facing
            // notification topic, and on a busy node it's hundreds of dirs that
            // would bury the real topics (and slow the walk).
            if sub.file_name().is_some_and(|n| n == "reply") {
                continue;
            }
            queue.push_back((sub, depth + 1));
        }
    }

    let mut infos = Vec::new();
    for (dir, name) in leaf_dirs {
        infos.push(stat_topic_dir(name, &dir).await);
    }
    infos.sort_by(|a, b| b.last_activity_ms.cmp(&a.last_activity_ms));
    Ok(infos)
}

/// Stat a single topic directory: count `.json` files + extract
/// the priority from the newest file.
async fn stat_topic_dir(name: String, dir: &PathBuf) -> TopicInfo {
    let mut message_count = 0usize;
    let mut newest_ms = 0i64;
    let mut newest_path: Option<PathBuf> = None;

    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                message_count += 1;
                if let Ok(meta) = entry.metadata().await {
                    if let Ok(modified) = meta.modified() {
                        let ms = modified
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        if ms > newest_ms {
                            newest_ms = ms;
                            newest_path = Some(path);
                        }
                    }
                }
            }
        }
    }

    let priority = if let Some(p) = newest_path {
        tokio::fs::read_to_string(&p)
            .await
            .ok()
            .and_then(|txt| serde_json::from_str::<serde_json::Value>(&txt).ok())
            .and_then(|v| v["priority"].as_str().map(String::from))
            .unwrap_or_else(|| "default".to_string())
    } else {
        String::new()
    };

    TopicInfo {
        name,
        message_count,
        last_activity_ms: newest_ms,
        priority,
    }
}

/// Load the 5 most-recent messages from a topic directory.
async fn load_topic_messages(topic: String) -> Result<Vec<TopicMessage>, String> {
    let root = bus_root().ok_or_else(|| "no data dir".to_string())?;
    // Topic dirs are directly under the bus root (no `topics/` segment).
    let topic_dir = root.join(std::path::Path::new(
        &topic.replace('/', &std::path::MAIN_SEPARATOR.to_string()),
    ));

    let mut files: Vec<(i64, PathBuf)> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&topic_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(meta) = entry.metadata().await {
                    if let Ok(modified) = meta.modified() {
                        let ms = modified
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        files.push((ms, path));
                    }
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(5);

    let mut messages = Vec::new();
    for (_ms, path) in files {
        if let Ok(txt) = tokio::fs::read_to_string(&path).await {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                let title = val["title"].as_str().unwrap_or("(no title)").to_string();
                let body = val["body"].as_str().unwrap_or("").to_string();
                let body_preview = if body.len() > 80 {
                    format!("{}…", &body[..80])
                } else {
                    body
                };
                let published_at_ms = val["ts_unix_ms"].as_i64().unwrap_or(0);
                let publisher = val["publisher"].as_str().unwrap_or("").to_string();
                messages.push(TopicMessage {
                    title,
                    body_preview,
                    published_at_ms,
                    publisher,
                });
            }
        }
    }
    Ok(messages)
}

/// Format a Unix-ms timestamp as a human-readable relative time string.
fn format_relative_time(ms: i64) -> String {
    if ms == 0 {
        return "never".to_string();
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let diff_s = (now_ms - ms).max(0) / 1000;
    if diff_s < 60 {
        "just now".to_string()
    } else if diff_s < 3600 {
        format!("{} min ago", diff_s / 60)
    } else if diff_s < 86400 {
        format!("{}h ago", diff_s / 3600)
    } else {
        format!("{}d ago", diff_s / 86400)
    }
}

/// Human "time left" for a snooze that expires at `until_unix_ms` (epoch ms).
/// Wraps [`snooze_remaining_at`] with the wall clock.
fn snooze_remaining(until_unix_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    snooze_remaining_at(until_unix_ms, now_ms)
}

/// Pure core of [`snooze_remaining`]: the remaining-time label at `now_ms`.
fn snooze_remaining_at(until_unix_ms: i64, now_ms: i64) -> String {
    let left_s = (until_unix_ms - now_ms) / 1000;
    if left_s <= 0 {
        "expired".to_string()
    } else if left_s < 3600 {
        format!("{}m left", (left_s / 60).max(1))
    } else if left_s < 86400 {
        format!("{}h left", left_s / 3600)
    } else {
        format!("{}d left", left_s / 86400)
    }
}

async fn load_subs() -> Result<(Vec<String>, Vec<String>), String> {
    let root = bus_root().ok_or_else(|| "no data dir".to_string())?;
    let path = root.join("subs.yaml");
    let txt = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let manifest: SubsYaml = serde_yaml::from_str(&txt).unwrap_or_default();
    Ok((manifest.topics, manifest.mute))
}

async fn sub_add(topic: String) -> Result<(), String> {
    let out = tokio::process::Command::new("mde-bus")
        .args(["sub", "add", &topic])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

async fn sub_remove(topic: String) -> Result<(), String> {
    let out = tokio::process::Command::new("mde-bus")
        .args(["sub", "remove", &topic])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

fn hooks_config_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("mde").join("bus-hooks.yaml"))
}

async fn load_hooks() -> Result<String, String> {
    let path = hooks_config_path().ok_or_else(|| "no config dir".to_string())?;
    tokio::fs::read_to_string(&path)
        .await
        .or_else(|_| Ok(String::new()))
}

async fn save_hooks(text: String) -> Result<(), String> {
    let path = hooks_config_path().ok_or_else(|| "no config dir".to_string())?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::write(&path, text.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

/// Copy a peer's subscriptions from the mesh-storage mount.
/// Looks for the peer's subs.yaml at
/// `~/.mde-mesh/<peer>/.local/share/mde/bus/subs.yaml`
/// (LizardFS per-peer home per MESHFS-4.1 mount layout).
async fn match_peer_subs(peer: String) -> Result<Vec<String>, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "no $HOME".to_string())?;
    let peer_subs = home
        .join(".mde-mesh")
        .join(&peer)
        .join(".local")
        .join("share")
        .join("mde")
        .join("bus")
        .join("subs.yaml");
    let txt = tokio::fs::read_to_string(&peer_subs)
        .await
        .map_err(|e| format!("peer @{peer} not reachable via mesh storage: {e}"))?;
    let manifest: SubsYaml = serde_yaml::from_str(&txt).map_err(|e| e.to_string())?;
    // Merge into local subs.yaml — add any topic not yet subscribed.
    let root = bus_root().ok_or_else(|| "no data dir".to_string())?;
    let local_path = root.join("subs.yaml");
    let local_txt = tokio::fs::read_to_string(&local_path)
        .await
        .unwrap_or_default();
    let mut local: SubsYaml = serde_yaml::from_str(&local_txt).unwrap_or_default();
    let mut added = Vec::new();
    for t in &manifest.topics {
        if !local.topics.contains(t) {
            local.topics.push(t.clone());
            added.push(t.clone());
        }
    }
    if !added.is_empty() {
        let yaml = serde_yaml::to_string(&local).map_err(|e| e.to_string())?;
        tokio::fs::write(&local_path, yaml)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(added)
}

async fn load_dnd() -> Result<(DndStatusJson, String, String), String> {
    // Read DND state via mde-bus CLI (JSON output).
    let out = tokio::process::Command::new("mde-bus")
        .args(["dnd", "status", "--json"])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let status: DndStatusJson = if out.status.success() && !out.stdout.is_empty() {
        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?
    } else {
        DndStatusJson::default()
    };

    // Read quiet hours from subs.yaml.
    let (qs, qe) = if let Some(root) = bus_root() {
        let path = root.join("subs.yaml");
        match tokio::fs::read_to_string(&path).await {
            Ok(txt) => {
                let manifest: SubsYaml = serde_yaml::from_str(&txt).unwrap_or_default();
                if let Some(qh) = manifest.quiet_hours {
                    (qh.start, qh.end)
                } else {
                    (String::new(), String::new())
                }
            }
            Err(_) => (String::new(), String::new()),
        }
    } else {
        (String::new(), String::new())
    };

    Ok((status, qs, qe))
}

async fn toggle_dnd() -> Result<(), String> {
    let out = tokio::process::Command::new("mde-bus")
        .args(["dnd", "toggle"])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

async fn save_quiet_hours(start: String, end: String) -> Result<(), String> {
    let root = bus_root().ok_or_else(|| "no data dir".to_string())?;
    let path = root.join("subs.yaml");

    let mut manifest: SubsYaml = match tokio::fs::read_to_string(&path).await {
        Ok(txt) => serde_yaml::from_str(&txt).unwrap_or_default(),
        Err(_) => SubsYaml::default(),
    };

    if start.is_empty() && end.is_empty() {
        manifest.quiet_hours = None;
    } else {
        manifest.quiet_hours = Some(QuietHoursYaml { start, end });
    }

    let yaml = serde_yaml::to_string(&manifest).map_err(|e| e.to_string())?;
    tokio::fs::write(&path, yaml)
        .await
        .map_err(|e| e.to_string())
}

// ─── Panel impl ───────────────────────────────────────────────────────────────

impl MeshBusPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// AUDIT-MESH-9 — load the default (Topics) tab when the panel opens.
    /// The per-tab fetch only fired on a `SelectTab` message, but opening the
    /// panel lands on Topics WITHOUT sending one, so a live bus rendered "No
    /// topics active yet". The nav dispatch calls this on open (mirroring every
    /// other panel's load-on-nav); it kicks the Topics walk so the registry
    /// shows real traffic immediately.
    pub fn load() -> Task<crate::Message> {
        Task::perform(load_topics(), |r| {
            crate::Message::MeshBus(Message::TopicsLoaded(r))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::SelectTab(tab) => {
                self.active_tab = tab;
                if tab == Tab::Topics && !self.topics.loaded && !self.topics.loading {
                    self.topics.loading = true;
                    return Task::perform(load_topics(), |r| {
                        crate::Message::MeshBus(Message::TopicsLoaded(r))
                    });
                }
                if tab == Tab::Subscriptions && !self.subs.loaded && !self.subs.loading {
                    self.subs.loading = true;
                    return Task::perform(load_subs(), |r| {
                        crate::Message::MeshBus(Message::SubsLoaded(r))
                    });
                }
                if tab == Tab::Dnd && !self.dnd.loaded && !self.dnd.loading {
                    self.dnd.loading = true;
                    return Task::perform(load_dnd(), |r| {
                        crate::Message::MeshBus(Message::DndLoaded(r))
                    });
                }
                if tab == Tab::Hooks && !self.hooks.loaded && !self.hooks.loading {
                    self.hooks.loading = true;
                    return Task::perform(load_hooks(), |r| {
                        crate::Message::MeshBus(Message::HooksLoaded(r))
                    });
                }
                Task::none()
            }

            Message::TopicsLoaded(result) => {
                self.topics.loading = false;
                self.topics.loaded = true;
                match result {
                    Ok(list) => {
                        self.topics.list = list;
                        self.topics.error = None;
                    }
                    Err(e) => {
                        self.topics.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::TopicSelected(name) => {
                if self.topics.expanded.as_deref() == Some(&name) {
                    self.topics.expanded = None;
                    self.topics.messages.clear();
                    return Task::none();
                }
                self.topics.expanded = Some(name.clone());
                self.topics.messages_loading = true;
                Task::perform(load_topic_messages(name), |r| {
                    crate::Message::MeshBus(Message::TopicMessagesLoaded(r))
                })
            }

            Message::TopicDeselected => {
                self.topics.expanded = None;
                self.topics.messages.clear();
                Task::none()
            }

            Message::TopicMessagesLoaded(result) => {
                self.topics.messages_loading = false;
                match result {
                    Ok(msgs) => self.topics.messages = msgs,
                    Err(_) => self.topics.messages.clear(),
                }
                Task::none()
            }

            Message::TopicsRefreshClicked => {
                self.topics.loaded = false;
                self.topics.loading = true;
                self.topics.expanded = None;
                self.topics.messages.clear();
                Task::perform(load_topics(), |r| {
                    crate::Message::MeshBus(Message::TopicsLoaded(r))
                })
            }

            Message::HooksLoaded(result) => {
                self.hooks.loading = false;
                self.hooks.loaded = true;
                match result {
                    Ok(txt) => {
                        self.hooks.content = text_editor::Content::with_text(&txt);
                        self.hooks.validate();
                    }
                    Err(e) => {
                        self.hooks.validation_error = Some(e);
                    }
                }
                Task::none()
            }

            Message::HooksEdited(action) => {
                self.hooks.content.perform(action);
                self.hooks.validate();
                Task::none()
            }

            Message::HooksSaveClicked => {
                if self.hooks.validation_error.is_some() {
                    return Task::none();
                }
                self.hooks.saving = true;
                let text = self.hooks.yaml_text();
                Task::perform(save_hooks(text), |r| {
                    crate::Message::MeshBus(Message::HooksSaveDone(r))
                })
            }

            Message::HooksSaveDone(result) => {
                self.hooks.saving = false;
                if let Err(e) = result {
                    self.hooks.validation_error = Some(e);
                }
                Task::none()
            }

            Message::HooksSampleInserted(idx) => {
                if let Some(sample) = HOOK_SAMPLES.get(idx) {
                    self.hooks.content = text_editor::Content::with_text(sample.yaml);
                    self.hooks.validate();
                }
                Task::none()
            }

            Message::SubsLoaded(result) => {
                self.subs.loading = false;
                self.subs.loaded = true;
                match result {
                    Ok((topics, muted)) => {
                        self.subs.topics = topics;
                        self.subs.muted = muted;
                        self.subs.error = None;
                    }
                    Err(e) => self.subs.error = Some(e),
                }
                Task::none()
            }

            Message::SubsNewTopicChanged(s) => {
                self.subs.new_topic = s;
                Task::none()
            }

            Message::SubsAddClicked => {
                let topic = self.subs.new_topic.trim().to_string();
                if topic.is_empty() {
                    return Task::none();
                }
                self.subs.new_topic.clear();
                Task::perform(sub_add(topic), |r| {
                    crate::Message::MeshBus(Message::SubsAddDone(r))
                })
            }

            Message::SubsAddDone(result) => match result {
                Ok(()) => {
                    self.subs.loaded = false;
                    self.subs.loading = true;
                    Task::perform(load_subs(), |r| {
                        crate::Message::MeshBus(Message::SubsLoaded(r))
                    })
                }
                Err(e) => {
                    self.subs.error = Some(e);
                    Task::none()
                }
            },

            Message::SubsRemoveClicked(topic) => Task::perform(sub_remove(topic), |r| {
                crate::Message::MeshBus(Message::SubsRemoveDone(r))
            }),

            Message::SubsRemoveDone(result) => match result {
                Ok(()) => {
                    self.subs.loaded = false;
                    self.subs.loading = true;
                    Task::perform(load_subs(), |r| {
                        crate::Message::MeshBus(Message::SubsLoaded(r))
                    })
                }
                Err(e) => {
                    self.subs.error = Some(e);
                    Task::none()
                }
            },

            Message::SubsPeerInputChanged(s) => {
                self.subs.peer_input = s;
                Task::none()
            }

            Message::SubsMatchPeerClicked => {
                let peer = self.subs.peer_input.trim().to_string();
                if peer.is_empty() {
                    return Task::none();
                }
                Task::perform(match_peer_subs(peer), |r| {
                    crate::Message::MeshBus(Message::SubsMatchPeerDone(r))
                })
            }

            Message::SubsMatchPeerDone(result) => match result {
                Ok(added) => {
                    self.subs.loaded = false;
                    self.subs.loading = true;
                    if added.is_empty() {
                        self.subs.error = Some("No new topics from that peer.".to_string());
                        self.subs.loading = false;
                        self.subs.loaded = true;
                        return Task::none();
                    }
                    Task::perform(load_subs(), |r| {
                        crate::Message::MeshBus(Message::SubsLoaded(r))
                    })
                }
                Err(e) => {
                    self.subs.error = Some(e);
                    Task::none()
                }
            },

            Message::SubsRefreshClicked => {
                self.subs.loaded = false;
                self.subs.loading = true;
                Task::perform(load_subs(), |r| {
                    crate::Message::MeshBus(Message::SubsLoaded(r))
                })
            }

            Message::DndLoaded(result) => {
                self.dnd.loading = false;
                self.dnd.loaded = true;
                match result {
                    Ok((status, qs, qe)) => {
                        self.dnd.quiet_start = qs.clone();
                        self.dnd.quiet_end = qe.clone();
                        self.dnd.quiet_start = qs;
                        self.dnd.quiet_end = qe;
                        self.dnd.status = Some(status);
                        self.dnd.error = None;
                    }
                    Err(e) => {
                        self.dnd.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::DndToggleClicked => {
                self.dnd.saving = true;
                Task::perform(toggle_dnd(), |r| {
                    crate::Message::MeshBus(Message::DndToggleDone(r))
                })
            }

            Message::DndToggleDone(result) => {
                self.dnd.saving = false;
                match result {
                    Ok(()) => {
                        self.dnd.loaded = false;
                        self.dnd.loading = true;
                        Task::perform(load_dnd(), |r| {
                            crate::Message::MeshBus(Message::DndLoaded(r))
                        })
                    }
                    Err(e) => {
                        self.dnd.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::DndRefreshClicked => {
                self.dnd.loaded = false;
                self.dnd.loading = true;
                Task::perform(load_dnd(), |r| {
                    crate::Message::MeshBus(Message::DndLoaded(r))
                })
            }

            Message::DndQuietStartChanged(s) => {
                self.dnd.quiet_start = s;
                Task::none()
            }

            Message::DndQuietEndChanged(s) => {
                self.dnd.quiet_end = s;
                Task::none()
            }

            Message::DndSaveClicked => {
                self.dnd.saving = true;
                let qs = self.dnd.quiet_start.clone();
                let qe = self.dnd.quiet_end.clone();
                Task::perform(save_quiet_hours(qs, qe), |r| {
                    crate::Message::MeshBus(Message::DndSaveDone(r))
                })
            }

            Message::DndSaveDone(result) => {
                self.dnd.saving = false;
                if let Err(e) = result {
                    self.dnd.error = Some(e);
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let sizes = FontSize::defaults();
        let radii = Radii::defaults();
        let accent = palette.accent.into_cosmic_color();
        let raised = palette.raised.into_cosmic_color();

        let title = text("Mackes Bus")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle = text("Per-peer notification distribution · ntfy over Nebula")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let tab_bar: Element<'_, crate::Message> = {
            let r = f32::from(radii.sm);
            let buttons: Vec<Element<'_, crate::Message>> = Tab::ALL
                .iter()
                .map(|&tab| {
                    let is_active = tab == self.active_tab;
                    let (bg, fg) = if is_active {
                        (accent, Color::WHITE)
                    } else {
                        (Color::TRANSPARENT, palette.text.into_cosmic_color())
                    };
                    button(
                        text(tab.label())
                            .size(TypeRole::Body.size_in(sizes))
                            .colr(fg),
                    )
                    .padding([6u16, 14u16])
                    .sty(move |_t: &Theme, status: ButtonStatus| {
                        let fill = match (is_active, status) {
                            (true, _) => bg,
                            (false, ButtonStatus::Hovered) => Color {
                                r: accent.r,
                                g: accent.g,
                                b: accent.b,
                                a: 0.08,
                            },
                            _ => bg,
                        };
                        button::Style {
                            snap: false,
                            background: Some(Background::Color(fill)),
                            text_color: fg,
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: r.into(),
                            },
                            shadow: cosmic::iced::Shadow::default(),
                            ..button::Style::default()
                        }
                    })
                    .on_press(crate::Message::MeshBus(Message::SelectTab(tab)))
                    .into()
                })
                .collect();

            row(buttons).spacing(4).into()
        };

        let tab_separator = {
            use cosmic::iced::widget::container;
            container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                .sty(move |_t: &Theme| cosmic::iced::widget::container::Style {
                    snap: false,
                    background: Some(Background::Color(raised)),
                    ..Default::default()
                })
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
        };

        let body: Element<'_, crate::Message> = match self.active_tab {
            Tab::Topics => self.view_topics_tab(palette, sizes),
            Tab::Subscriptions => self.view_subscriptions_tab(palette, sizes),
            Tab::Hooks => self.view_hooks_tab(palette, sizes),
            Tab::Audit => empty_state(
                EmptyState::info(
                    "No audit events recorded",
                    "Bus activity will appear here as messages flow through the broker.",
                )
                .with_icon(Icon::History),
                palette,
                || crate::Message::Noop,
            ),
            Tab::Dnd => self.view_dnd_tab(palette, sizes),
        };

        let header = column![title, subtitle].spacing(4);

        let content = column![
            header,
            Space::new().height(12),
            tab_bar,
            tab_separator,
            Space::new().height(16),
            body,
        ]
        .spacing(0)
        .align_x(alignment::Horizontal::Left);

        panel_container(content.into(), density)
    }

    fn view_topics_tab(&self, palette: Palette, sizes: FontSize) -> Element<'_, crate::Message> {
        if self.topics.loading {
            return text("Loading…")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into();
        }

        if !self.topics.loaded || self.topics.list.is_empty() {
            return empty_state(
                EmptyState::info(
                    "No topics active yet",
                    "Publish a message or start a webhook to create the first topic.",
                )
                .with_icon(Icon::Notification),
                palette,
                || crate::Message::Noop,
            );
        }

        let radii = Radii::defaults();
        let r = f32::from(radii.sm);
        let raised = palette.raised.into_cosmic_color();
        let danger = palette.danger.into_cosmic_color();

        // Build the cascading topic list.
        let mut items: Vec<Element<'_, crate::Message>> = Vec::new();

        for info in &self.topics.list {
            let is_expanded = self.topics.expanded.as_deref() == Some(info.name.as_str());
            let card_state = if is_expanded {
                CardState::Selected
            } else {
                CardState::Default
            };

            let subtitle = {
                let count_str = if info.message_count == 1 {
                    "1 message".to_string()
                } else {
                    format!("{} messages", info.message_count)
                };
                let pri = if info.priority.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", info.priority)
                };
                let age = format!(" · {}", format_relative_time(info.last_activity_ms));
                format!("{count_str}{pri}{age}")
            };

            let card = ObjectCard::small(Icon::Notification, info.name.as_str())
                .with_subtitle(subtitle)
                .with_state(card_state);

            let topic_name = info.name.clone();
            let press_msg = if is_expanded {
                crate::Message::MeshBus(Message::TopicDeselected)
            } else {
                crate::Message::MeshBus(Message::TopicSelected(topic_name))
            };

            let card_btn: Element<'_, crate::Message> = button(object_card(card, palette))
                .padding(0)
                .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
                    snap: false,
                    background: None,
                    text_color: Color::TRANSPARENT,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: r.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..button::Style::default()
                })
                .on_press(press_msg)
                .into();

            items.push(card_btn);

            // Cascade: message list below the selected card.
            if is_expanded {
                if self.topics.messages_loading {
                    items.push(
                        text("Loading messages…")
                            .size(TypeRole::Caption.size_in(sizes))
                            .colr(palette.text_muted.into_cosmic_color())
                            .into(),
                    );
                } else if self.topics.messages.is_empty() {
                    items.push(
                        text("No messages stored for this topic yet.")
                            .size(TypeRole::Caption.size_in(sizes))
                            .colr(palette.text_muted.into_cosmic_color())
                            .into(),
                    );
                } else {
                    use cosmic::iced::widget::container;
                    let msg_rows: Vec<Element<'_, crate::Message>> = self
                        .topics
                        .messages
                        .iter()
                        .map(|m| {
                            let age = format_relative_time(m.published_at_ms);
                            let by = if m.publisher.is_empty() {
                                String::new()
                            } else {
                                format!(" · @{}", m.publisher)
                            };
                            column![
                                text(m.title.as_str())
                                    .size(TypeRole::Body.size_in(sizes))
                                    .colr(palette.text.into_cosmic_color()),
                                text(format!("{}{} · {}", age, by, m.body_preview.as_str()))
                                    .size(TypeRole::Caption.size_in(sizes))
                                    .colr(palette.text_muted.into_cosmic_color()),
                            ]
                            .spacing(2)
                            .into()
                        })
                        .collect();
                    let msg_list = column(msg_rows).spacing(10);
                    items.push(
                        container(msg_list)
                            .padding(cosmic::iced::Padding {
                                top: 8.0,
                                right: 16.0,
                                bottom: 8.0,
                                left: 16.0,
                            })
                            .sty(move |_t: &Theme| cosmic::iced::widget::container::Style {
                                snap: false,
                                background: Some(Background::Color(raised)),
                                border: Border {
                                    color: Color::TRANSPARENT,
                                    width: 0.0,
                                    radius: r.into(),
                                },
                                ..Default::default()
                            })
                            .width(Length::Fill)
                            .into(),
                    );
                }
                items.push(Space::new().height(4).into());
            }
        }

        if let Some(e) = &self.topics.error {
            items.push(
                text(format!("Error: {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(danger)
                    .into(),
            );
        }

        column(items).spacing(4).into()
    }

    fn view_subscriptions_tab(
        &self,
        palette: Palette,
        sizes: FontSize,
    ) -> Element<'_, crate::Message> {
        if self.subs.loading {
            return text("Loading…")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into();
        }

        if !self.subs.loaded {
            return empty_state(
                EmptyState::info(
                    "No subscriptions configured",
                    "Add a subscription to start receiving messages on this peer.",
                )
                .with_icon(Icon::Network),
                palette,
                || crate::Message::Noop,
            );
        }

        let accent = palette.accent.into_cosmic_color();
        let danger = palette.danger.into_cosmic_color();
        let danger_fill = Color { a: 0.12, ..danger };
        let radii = Radii::defaults();
        let r = f32::from(radii.sm);

        // — Topic list —
        let list_label = text(format!("Subscriptions ({})", self.subs.topics.len()))
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let topic_rows: Vec<Element<'_, crate::Message>> = if self.subs.topics.is_empty() {
            vec![text("No topics subscribed yet.")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into()]
        } else {
            self.subs
                .topics
                .iter()
                .map(|t| {
                    let topic = t.clone();
                    let is_muted = self.subs.muted.contains(&topic);
                    let label_color = if is_muted {
                        palette.text_muted.into_cosmic_color()
                    } else {
                        palette.text.into_cosmic_color()
                    };
                    let mute_note: Option<Element<'_, crate::Message>> = if is_muted {
                        Some(
                            text("muted")
                                .size(TypeRole::Caption.size_in(sizes))
                                .colr(palette.text_muted.into_cosmic_color())
                                .into(),
                        )
                    } else {
                        None
                    };
                    let remove_topic = topic.clone();
                    let remove_btn: Element<'_, crate::Message> = button(
                        text("Remove")
                            .size(TypeRole::Caption.size_in(sizes))
                            .colr(label_color),
                    )
                    .padding([2u16, 8u16])
                    .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
                        snap: false,
                        background: Some(Background::Color(danger_fill)),
                        text_color: danger,
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: r.into(),
                        },
                        shadow: cosmic::iced::Shadow::default(),
                        ..button::Style::default()
                    })
                    .on_press(crate::Message::MeshBus(Message::SubsRemoveClicked(
                        remove_topic,
                    )))
                    .into();

                    let mut row_items: Vec<Element<'_, crate::Message>> = vec![text(t.as_str())
                        .size(TypeRole::Body.size_in(sizes))
                        .colr(label_color)
                        .into()];
                    if let Some(mn) = mute_note {
                        row_items.push(Space::new().width(8).into());
                        row_items.push(mn);
                    }
                    row_items.push(Space::new().width(Length::Fill).into());
                    row_items.push(remove_btn);

                    row(row_items)
                        .align_y(cosmic::iced::Alignment::Center)
                        .into()
                })
                .collect()
        };

        let topic_list: Element<'_, crate::Message> = column(topic_rows).spacing(6).into();

        // — Add topic row —
        let add_input: Element<'_, crate::Message> =
            text_input("fleet/# or mon/+/alerts", &self.subs.new_topic)
                .on_input(|s| crate::Message::MeshBus(Message::SubsNewTopicChanged(s)))
                .on_submit(crate::Message::MeshBus(Message::SubsAddClicked))
                .width(Length::Fixed(240.0))
                .into();

        let add_btn: Element<'_, crate::Message> = button(
            text("Subscribe")
                .size(TypeRole::Body.size_in(sizes))
                .colr(Color::WHITE),
        )
        .padding([6u16, 14u16])
        .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(accent)),
            text_color: Color::WHITE,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            ..button::Style::default()
        })
        .on_press(crate::Message::MeshBus(Message::SubsAddClicked))
        .into();

        let add_row: Element<'_, crate::Message> = row![add_input, Space::new().width(8), add_btn,]
            .align_y(cosmic::iced::Alignment::Center)
            .into();

        // — Match @peer section —
        let peer_label = text("Copy from peer")
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let peer_hint =
            text("Copies all subscriptions from another peer's subs.yaml via mesh storage.")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color());

        let peer_input: Element<'_, crate::Message> = text_input("hostname", &self.subs.peer_input)
            .on_input(|s| crate::Message::MeshBus(Message::SubsPeerInputChanged(s)))
            .on_submit(crate::Message::MeshBus(Message::SubsMatchPeerClicked))
            .width(Length::Fixed(160.0))
            .into();

        let match_btn: Element<'_, crate::Message> = button(
            text("Match @peer")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
        )
        .padding([6u16, 14u16])
        .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(palette.raised.into_cosmic_color())),
            text_color: palette.text.into_cosmic_color(),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            ..button::Style::default()
        })
        .on_press(crate::Message::MeshBus(Message::SubsMatchPeerClicked))
        .into();

        let peer_row: Element<'_, crate::Message> =
            row![peer_input, Space::new().width(8), match_btn,]
                .align_y(cosmic::iced::Alignment::Center)
                .into();

        // — Error display —
        let error_row: Option<Element<'_, crate::Message>> = self.subs.error.as_deref().map(|e| {
            text(format!("Error: {e}"))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(danger)
                .into()
        });

        let mut col = column![
            list_label,
            Space::new().height(8),
            topic_list,
            Space::new().height(16),
            add_row,
            Space::new().height(28),
            peer_label,
            Space::new().height(4),
            peer_hint,
            Space::new().height(8),
            peer_row,
        ]
        .spacing(0);

        if let Some(err) = error_row {
            col = col.push(Space::new().height(12)).push(err);
        }

        col.into()
    }

    fn view_dnd_tab(&self, palette: Palette, sizes: FontSize) -> Element<'_, crate::Message> {
        if self.dnd.loading {
            return text("Loading…")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into();
        }

        let accent = palette.accent.into_cosmic_color();
        let danger = palette.danger.into_cosmic_color();
        let radii = Radii::defaults();
        let r = f32::from(radii.sm);

        // — DND master toggle —
        let (active, since_label, peer_label) = match &self.dnd.status {
            Some(s) => {
                let since = if s.since_unix_ms > 0 {
                    format!("since {}", s.since_unix_ms / 1000)
                } else {
                    String::new()
                };
                let by = if s.set_by_peer.is_empty() {
                    String::new()
                } else {
                    format!("by @{}", s.set_by_peer)
                };
                (s.active, since, by)
            }
            None => (false, String::new(), String::new()),
        };

        let toggle_label = if active { "DND On" } else { "DND Off" };
        let toggle_bg = if active {
            accent
        } else {
            palette.raised.into_cosmic_color()
        };
        let toggle_fg = if active {
            Color::WHITE
        } else {
            palette.text.into_cosmic_color()
        };

        let toggle_btn: Element<'_, crate::Message> = button(
            text(toggle_label)
                .size(TypeRole::Body.size_in(sizes))
                .colr(toggle_fg),
        )
        .padding([8u16, 20u16])
        .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(toggle_bg)),
            text_color: toggle_fg,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            ..button::Style::default()
        })
        .on_press(crate::Message::MeshBus(Message::DndToggleClicked))
        .into();

        let meta_parts: Vec<&str> = [since_label.as_str(), peer_label.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let meta_str = meta_parts.join(" · ");

        let toggle_row: Element<'_, crate::Message> = if meta_str.is_empty() {
            toggle_btn
        } else {
            row![
                toggle_btn,
                Space::new().width(12),
                text(meta_str)
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .align_y(cosmic::iced::Alignment::Center)
            .into()
        };

        // — Quiet hours editor —
        let quiet_label = text("Global quiet window")
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let quiet_hint = text(
            "Messages delivered outside this window only. Leave blank to deliver around the clock.",
        )
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

        let start_input: Element<'_, crate::Message> = text_input("22:00", &self.dnd.quiet_start)
            .on_input(|s| crate::Message::MeshBus(Message::DndQuietStartChanged(s)))
            .width(Length::Fixed(80.0))
            .into();

        let end_input: Element<'_, crate::Message> = text_input("07:00", &self.dnd.quiet_end)
            .on_input(|s| crate::Message::MeshBus(Message::DndQuietEndChanged(s)))
            .width(Length::Fixed(80.0))
            .into();

        let save_bg = accent;
        let save_fg = Color::WHITE;
        let save_btn: Element<'_, crate::Message> = button(
            text(if self.dnd.saving {
                "Applying…"
            } else {
                "Apply"
            })
            .size(TypeRole::Body.size_in(sizes))
            .colr(save_fg),
        )
        .padding([6u16, 16u16])
        .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
            snap: false,
            background: Some(Background::Color(save_bg)),
            text_color: save_fg,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: r.into(),
            },
            shadow: cosmic::iced::Shadow::default(),
            ..button::Style::default()
        })
        .on_press(crate::Message::MeshBus(Message::DndSaveClicked))
        .into();

        let quiet_row: Element<'_, crate::Message> = row![
            start_input,
            Space::new().width(8),
            text("→")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
            Space::new().width(8),
            end_input,
            Space::new().width(12),
            save_btn,
        ]
        .align_y(cosmic::iced::Alignment::Center)
        .into();

        // — Active snoozes —
        let snooze_count = self
            .dnd
            .status
            .as_ref()
            .map(|s| s.snoozes.len())
            .unwrap_or(0);

        let snooze_label = text(format!("Active fleet snoozes ({snooze_count})"))
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let snooze_body: Element<'_, crate::Message> = if snooze_count == 0 {
            text("No active snoozes — use `mde-bus mute <topic> --duration <D>` to snooze.")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            let rows: Vec<Element<'_, crate::Message>> = self
                .dnd
                .status
                .as_ref()
                .map(|s| &s.snoozes)
                .unwrap_or(&vec![])
                .iter()
                .map(|sn| {
                    let by = if sn.set_by_peer.is_empty() {
                        String::new()
                    } else {
                        format!(" (by @{})", sn.set_by_peer)
                    };
                    text(format!(
                        "{}{} · {}",
                        sn.topic,
                        by,
                        snooze_remaining(sn.until_unix_ms)
                    ))
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text.into_cosmic_color())
                    .into()
                })
                .collect();
            column(rows).spacing(4).into()
        };

        // — Error display —
        let error_row: Option<Element<'_, crate::Message>> = self.dnd.error.as_deref().map(|e| {
            text(format!("Error: {e}"))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(danger)
                .into()
        });

        let mut col = column![
            toggle_row,
            Space::new().height(20),
            quiet_label,
            Space::new().height(4),
            quiet_hint,
            Space::new().height(8),
            quiet_row,
            Space::new().height(24),
            snooze_label,
            Space::new().height(8),
            snooze_body,
        ]
        .spacing(0);

        if let Some(err) = error_row {
            col = col.push(Space::new().height(12)).push(err);
        }

        col.into()
    }

    fn view_hooks_tab(&self, palette: Palette, sizes: FontSize) -> Element<'_, crate::Message> {
        if self.hooks.loading {
            return text("Loading…")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into();
        }

        let accent = palette.accent.into_cosmic_color();
        let danger = palette.danger.into_cosmic_color();
        let radii = Radii::defaults();
        let r = f32::from(radii.sm);

        // — Editor —
        let editor: Element<'_, crate::Message> = text_editor(&self.hooks.content)
            .height(Length::Fixed(280.0))
            .on_action(|a| crate::Message::MeshBus(Message::HooksEdited(a)))
            .into();

        // — Sample insert buttons —
        let mut sample_row_items: Vec<Element<'_, crate::Message>> = vec![
            text("Insert sample:")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
            Space::new().width(8).into(),
        ];
        for (i, s) in HOOK_SAMPLES.iter().enumerate() {
            sample_row_items.push(
                button(
                    text(s.label)
                        .size(TypeRole::Caption.size_in(sizes))
                        .colr(palette.text.into_cosmic_color()),
                )
                .padding([4u16, 10u16])
                .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
                    snap: false,
                    background: Some(Background::Color(palette.raised.into_cosmic_color())),
                    text_color: palette.text.into_cosmic_color(),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: r.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..button::Style::default()
                })
                .on_press(crate::Message::MeshBus(Message::HooksSampleInserted(i)))
                .into(),
            );
        }
        let sample_row: Element<'_, crate::Message> = row(sample_row_items)
            .spacing(6)
            .align_y(cosmic::iced::Alignment::Center)
            .into();

        // — Apply button —
        let has_error = self.hooks.validation_error.is_some();
        let apply_bg = if has_error {
            palette.raised.into_cosmic_color()
        } else {
            accent
        };
        let apply_fg = if has_error {
            palette.text_muted.into_cosmic_color()
        } else {
            Color::WHITE
        };
        let apply_label = if self.hooks.saving {
            "Applying…"
        } else {
            "Apply"
        };

        let apply_btn: Element<'_, crate::Message> = if has_error || self.hooks.saving {
            button(
                text(apply_label)
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(apply_fg),
            )
            .padding([6u16, 16u16])
            .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
                snap: false,
                background: Some(Background::Color(apply_bg)),
                text_color: apply_fg,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: r.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..button::Style::default()
            })
            .into()
        } else {
            button(
                text(apply_label)
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(apply_fg),
            )
            .padding([6u16, 16u16])
            .sty(move |_t: &Theme, _s: ButtonStatus| button::Style {
                snap: false,
                background: Some(Background::Color(apply_bg)),
                text_color: apply_fg,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: r.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..button::Style::default()
            })
            .on_press(crate::Message::MeshBus(Message::HooksSaveClicked))
            .into()
        };

        // Build column — validation error (if any) appears between editor and samples.
        let mut items: Vec<Element<'_, crate::Message>> = vec![editor];
        if let Some(e) = &self.hooks.validation_error {
            items.push(Space::new().height(6).into());
            items.push(
                text(format!("⚠ {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(danger)
                    .into(),
            );
        }
        items.push(Space::new().height(8).into());
        items.push(sample_row);
        items.push(Space::new().height(12).into());
        items.push(apply_btn);

        column(items).spacing(0).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_remaining_labels() {
        let now = 1_000_000_000_000_i64;
        assert_eq!(snooze_remaining_at(now - 5_000, now), "expired");
        assert_eq!(snooze_remaining_at(now, now), "expired");
        assert_eq!(snooze_remaining_at(now + 90_000, now), "1m left"); // 1.5 min
        assert_eq!(snooze_remaining_at(now + 1_800_000, now), "30m left");
        assert_eq!(snooze_remaining_at(now + 7_200_000, now), "2h left");
        assert_eq!(snooze_remaining_at(now + 172_800_000, now), "2d left");
    }

    #[test]
    fn default_tab_is_topics() {
        let panel = MeshBusPanel::new();
        assert_eq!(panel.active_tab, Tab::Topics);
    }

    #[test]
    fn select_tab_updates_active() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Subscriptions));
        assert_eq!(panel.active_tab, Tab::Subscriptions);
        let _ = panel.update(Message::SelectTab(Tab::Dnd));
        assert_eq!(panel.active_tab, Tab::Dnd);
    }

    #[test]
    fn all_tabs_cycle_without_panic() {
        let mut panel = MeshBusPanel::new();
        for tab in Tab::ALL {
            let _ = panel.update(Message::SelectTab(tab));
            assert_eq!(panel.active_tab, tab);
        }
    }

    #[test]
    fn tab_labels_are_non_empty() {
        for tab in Tab::ALL {
            assert!(!tab.label().is_empty());
        }
    }

    #[test]
    fn five_tabs_declared() {
        assert_eq!(Tab::ALL.len(), 5);
    }

    #[test]
    fn dnd_not_loaded_on_topics_tab() {
        let mut panel = MeshBusPanel::new();
        // Switching to Topics does not trigger a DND load.
        let _ = panel.update(Message::SelectTab(Tab::Topics));
        assert!(!panel.dnd.loaded);
        assert!(!panel.dnd.loading);
    }

    #[test]
    fn dnd_loading_set_on_dnd_tab_switch() {
        let mut panel = MeshBusPanel::new();
        // Switching to DND triggers loading (returns a Task::perform).
        let _ = panel.update(Message::SelectTab(Tab::Dnd));
        assert!(panel.dnd.loading);
        assert!(!panel.dnd.loaded);
    }

    #[test]
    fn dnd_loaded_ok_populates_state() {
        let mut panel = MeshBusPanel::new();
        let status = DndStatusJson {
            active: true,
            since_unix_ms: 1_700_000_000_000,
            set_by_peer: "desktop-2".to_string(),
            snoozes: vec![],
        };
        let _ = panel.update(Message::DndLoaded(Ok((
            status,
            "22:00".to_string(),
            "07:00".to_string(),
        ))));
        let s = panel.dnd.status.as_ref().unwrap();
        assert!(s.active);
        assert_eq!(s.set_by_peer, "desktop-2");
        assert_eq!(panel.dnd.quiet_start, "22:00");
        assert_eq!(panel.dnd.quiet_end, "07:00");
        assert!(panel.dnd.loaded);
        assert!(!panel.dnd.loading);
        assert!(panel.dnd.error.is_none());
    }

    #[test]
    fn dnd_loaded_err_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::DndLoaded(Err("mde-bus not found".to_string())));
        assert!(panel.dnd.error.is_some());
        assert!(panel.dnd.status.is_none());
        assert!(panel.dnd.loaded);
    }

    #[test]
    fn quiet_hours_inputs_update_state() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::DndQuietStartChanged("23:00".to_string()));
        assert_eq!(panel.dnd.quiet_start, "23:00");
        let _ = panel.update(Message::DndQuietEndChanged("06:00".to_string()));
        assert_eq!(panel.dnd.quiet_end, "06:00");
    }

    #[test]
    fn toggle_sets_saving_flag() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::DndToggleClicked);
        assert!(panel.dnd.saving);
    }

    // ─── Subscriptions tab tests ──────────────────────────────────────────────

    #[test]
    fn subs_loading_set_on_subscriptions_tab_switch() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Subscriptions));
        assert!(panel.subs.loading);
        assert!(!panel.subs.loaded);
    }

    #[test]
    fn subs_not_loaded_on_topics_tab() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Topics));
        assert!(!panel.subs.loaded);
        assert!(!panel.subs.loading);
    }

    #[test]
    fn subs_loaded_ok_populates_state() {
        let mut panel = MeshBusPanel::new();
        let topics = vec!["fleet/#".to_string(), "mon/+/alerts".to_string()];
        let muted = vec!["fleet/debug".to_string()];
        let _ = panel.update(Message::SubsLoaded(Ok((topics.clone(), muted.clone()))));
        assert_eq!(panel.subs.topics, topics);
        assert_eq!(panel.subs.muted, muted);
        assert!(panel.subs.loaded);
        assert!(!panel.subs.loading);
        assert!(panel.subs.error.is_none());
    }

    #[test]
    fn subs_loaded_err_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SubsLoaded(Err("no data dir".to_string())));
        assert!(panel.subs.error.is_some());
        assert!(panel.subs.topics.is_empty());
        assert!(panel.subs.loaded);
    }

    #[test]
    fn subs_new_topic_changed_updates_state() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SubsNewTopicChanged("gh/#".to_string()));
        assert_eq!(panel.subs.new_topic, "gh/#");
    }

    #[test]
    fn subs_add_clears_input_and_triggers_task() {
        let mut panel = MeshBusPanel::new();
        panel.subs.new_topic = "gh/#".to_string();
        let _ = panel.update(Message::SubsAddClicked);
        // Input cleared immediately.
        assert!(panel.subs.new_topic.is_empty());
    }

    #[test]
    fn subs_add_noop_on_empty_input() {
        let mut panel = MeshBusPanel::new();
        panel.subs.new_topic = String::new();
        let _ = panel.update(Message::SubsAddClicked);
        // No state change — still not loading.
        assert!(!panel.subs.loading);
    }

    #[test]
    fn subs_peer_input_changed_updates_state() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SubsPeerInputChanged("desktop-2".to_string()));
        assert_eq!(panel.subs.peer_input, "desktop-2");
    }

    #[test]
    fn subs_remove_done_error_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SubsRemoveDone(Err("failed".to_string())));
        assert!(panel.subs.error.is_some());
    }

    #[test]
    fn subs_match_peer_done_empty_sets_info_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SubsMatchPeerDone(Ok(vec![])));
        assert!(panel.subs.error.is_some());
        assert!(panel.subs.loaded);
    }

    // ─── Topics tab tests ─────────────────────────────────────────────────────

    #[test]
    fn topics_loading_set_on_topics_tab_switch() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Topics));
        assert!(panel.topics.loading);
        assert!(!panel.topics.loaded);
    }

    #[test]
    fn topics_not_loaded_on_dnd_tab() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Dnd));
        assert!(!panel.topics.loaded);
        assert!(!panel.topics.loading);
    }

    #[test]
    fn topics_loaded_ok_populates_list() {
        let mut panel = MeshBusPanel::new();
        let list = vec![TopicInfo {
            name: "fleet/announce".to_string(),
            message_count: 3,
            last_activity_ms: 1_700_000_000_000,
            priority: "default".to_string(),
        }];
        let _ = panel.update(Message::TopicsLoaded(Ok(list.clone())));
        assert!(panel.topics.loaded);
        assert!(!panel.topics.loading);
        assert_eq!(panel.topics.list.len(), 1);
        assert_eq!(panel.topics.list[0].name, "fleet/announce");
        assert!(panel.topics.error.is_none());
    }

    #[test]
    fn topics_loaded_err_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::TopicsLoaded(Err("no data dir".to_string())));
        assert!(panel.topics.error.is_some());
        assert!(panel.topics.list.is_empty());
        assert!(panel.topics.loaded);
    }

    #[test]
    fn topic_selected_sets_expanded_and_triggers_messages_load() {
        let mut panel = MeshBusPanel::new();
        panel.topics.loaded = true;
        panel.topics.list = vec![TopicInfo {
            name: "fleet/announce".to_string(),
            ..Default::default()
        }];
        let _ = panel.update(Message::TopicSelected("fleet/announce".to_string()));
        assert_eq!(panel.topics.expanded.as_deref(), Some("fleet/announce"));
        assert!(panel.topics.messages_loading);
    }

    #[test]
    fn topic_selected_twice_deselects() {
        let mut panel = MeshBusPanel::new();
        panel.topics.expanded = Some("fleet/announce".to_string());
        let _ = panel.update(Message::TopicSelected("fleet/announce".to_string()));
        assert!(panel.topics.expanded.is_none());
        assert!(!panel.topics.messages_loading);
    }

    #[test]
    fn topic_deselected_clears_expanded() {
        let mut panel = MeshBusPanel::new();
        panel.topics.expanded = Some("mon/cpu".to_string());
        panel.topics.messages = vec![TopicMessage {
            title: "CPU spike".to_string(),
            ..Default::default()
        }];
        let _ = panel.update(Message::TopicDeselected);
        assert!(panel.topics.expanded.is_none());
        assert!(panel.topics.messages.is_empty());
    }

    #[test]
    fn topic_messages_loaded_ok_populates_messages() {
        let mut panel = MeshBusPanel::new();
        panel.topics.messages_loading = true;
        let msgs = vec![TopicMessage {
            title: "Alert fired".to_string(),
            body_preview: "CPU > 90%".to_string(),
            published_at_ms: 1_700_000_000_000,
            publisher: "desktop-1".to_string(),
        }];
        let _ = panel.update(Message::TopicMessagesLoaded(Ok(msgs)));
        assert!(!panel.topics.messages_loading);
        assert_eq!(panel.topics.messages.len(), 1);
        assert_eq!(panel.topics.messages[0].title, "Alert fired");
    }

    #[test]
    fn topic_messages_loaded_err_clears_messages() {
        let mut panel = MeshBusPanel::new();
        panel.topics.messages_loading = true;
        panel.topics.messages = vec![TopicMessage::default()];
        let _ = panel.update(Message::TopicMessagesLoaded(Err("io error".to_string())));
        assert!(!panel.topics.messages_loading);
        assert!(panel.topics.messages.is_empty());
    }

    #[test]
    fn topics_refresh_clears_and_reloads() {
        let mut panel = MeshBusPanel::new();
        panel.topics.loaded = true;
        panel.topics.expanded = Some("fleet/sec".to_string());
        panel.topics.list = vec![TopicInfo::default()];
        let _ = panel.update(Message::TopicsRefreshClicked);
        assert!(panel.topics.loading);
        assert!(!panel.topics.loaded);
        assert!(panel.topics.expanded.is_none());
        assert!(panel.topics.messages.is_empty());
    }

    #[test]
    fn format_relative_time_zero_is_never() {
        assert_eq!(format_relative_time(0), "never");
    }

    #[test]
    fn format_relative_time_recent_is_just_now() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        assert_eq!(format_relative_time(now_ms - 5_000), "just now");
    }

    // ─── Hooks tab tests ──────────────────────────────────────────────────────

    #[test]
    fn hooks_loading_set_on_hooks_tab_switch() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Hooks));
        assert!(panel.hooks.loading);
        assert!(!panel.hooks.loaded);
    }

    #[test]
    fn hooks_not_loaded_on_topics_tab() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::SelectTab(Tab::Topics));
        assert!(!panel.hooks.loaded);
        assert!(!panel.hooks.loading);
    }

    #[test]
    fn hooks_loaded_ok_populates_content() {
        let mut panel = MeshBusPanel::new();
        let yaml = "adapters:\n  github:\n    rules: []\n".to_string();
        let _ = panel.update(Message::HooksLoaded(Ok(yaml.clone())));
        assert!(panel.hooks.loaded);
        assert!(!panel.hooks.loading);
        assert_eq!(panel.hooks.yaml_text(), yaml);
        assert!(panel.hooks.validation_error.is_none());
    }

    #[test]
    fn hooks_loaded_valid_yaml_no_adapters_key_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::HooksLoaded(Ok("other: value\n".to_string())));
        assert!(panel.hooks.validation_error.is_some());
    }

    #[test]
    fn hooks_loaded_invalid_yaml_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::HooksLoaded(Ok(": bad yaml :::".to_string())));
        assert!(panel.hooks.validation_error.is_some());
    }

    #[test]
    fn hooks_loaded_err_sets_error() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::HooksLoaded(Err("no config dir".to_string())));
        assert!(panel.hooks.validation_error.is_some());
        assert!(panel.hooks.loaded);
    }

    #[test]
    fn hooks_save_blocked_when_validation_error_present() {
        let mut panel = MeshBusPanel::new();
        panel.hooks.loaded = true;
        panel.hooks.validation_error = Some("bad yaml".to_string());
        let _ = panel.update(Message::HooksSaveClicked);
        // saving must NOT be set — the handler bails early on validation error.
        assert!(!panel.hooks.saving);
    }

    #[test]
    fn hooks_save_clicked_sets_saving() {
        let mut panel = MeshBusPanel::new();
        panel.hooks.loaded = true;
        panel.hooks.content = text_editor::Content::with_text("adapters:\n  x:\n    rules: []\n");
        let _ = panel.update(Message::HooksLoaded(Ok(
            "adapters:\n  x:\n    rules: []\n".to_string()
        )));
        let _ = panel.update(Message::HooksSaveClicked);
        assert!(panel.hooks.saving);
    }

    #[test]
    fn hooks_save_done_ok_clears_saving() {
        let mut panel = MeshBusPanel::new();
        panel.hooks.saving = true;
        let _ = panel.update(Message::HooksSaveDone(Ok(())));
        assert!(!panel.hooks.saving);
        assert!(panel.hooks.validation_error.is_none());
    }

    #[test]
    fn hooks_save_done_err_sets_error() {
        let mut panel = MeshBusPanel::new();
        panel.hooks.saving = true;
        let _ = panel.update(Message::HooksSaveDone(Err("write failed".to_string())));
        assert!(!panel.hooks.saving);
        assert!(panel.hooks.validation_error.is_some());
    }

    #[test]
    fn hooks_sample_inserted_updates_content() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::HooksSampleInserted(0));
        let txt = panel.hooks.yaml_text();
        assert!(txt.contains("adapters:"));
        // GitHub sample should reference the github adapter.
        assert!(txt.contains("github"));
        assert!(panel.hooks.validation_error.is_none());
    }

    #[test]
    fn hooks_sample_inserted_oob_is_noop() {
        let mut panel = MeshBusPanel::new();
        let _ = panel.update(Message::HooksSampleInserted(99));
        // No panic, content stays empty.
        assert!(panel.hooks.yaml_text().trim().is_empty());
    }

    #[test]
    fn hooks_tab_state_clone_preserves_fields() {
        let mut state = HooksTabState::default();
        state.validation_error = Some("err".to_string());
        state.loading = true;
        let cloned = state.clone();
        assert_eq!(cloned.validation_error, state.validation_error);
        assert_eq!(cloned.loading, state.loading);
    }
}
