//! NOTIFY-3 — the MDE-Notification-Hub **center**: a layer-shell slide-out
//! listing the live mesh + desktop alert stream, grouped by source and colored
//! by severity (design: `docs/design/mde-notification-hub.md`).
//!
//! An Overlay-layer surface anchored to the right edge (the `mde-mesh-wallpaper`
//! layer-shell pattern, but interactive — `OnDemand` keyboard so its buttons
//! click). It polls the [`mde_notify::AlertTail`] over the live system bus
//! (`mde_bus::client_data_dir`) on a cadence; each new alert appears in its
//! source group. Collapsible groups + mark-all-read + clear-all. Renders
//! entirely through `mde-theme` Carbon tokens (§4 — no raw hex).
//!
//! The model + bus tail + severity/source classification live in the
//! render-agnostic `mde-notify` crate; this binary is the libcosmic glue.

use std::collections::HashSet;

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    get_layer_surface, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{window, Element, Length, Padding, Subscription, Task, Theme};
use mackes_mesh_types::lighthouse::{self, Beacon};
use mde_notify::{severity_token, AlertItem, AlertTail, Severity, Source};
use mde_theme::Palette;
// World-2 (raw `cosmic::iced`) layer-shell daemon — use the iced widgets +
// raw `.color()` directly; only borrow the Rgba→Color conversion shim (the
// `.colr`/`.sty` extensions are world-1 `cosmic::Theme`-bound and don't apply).
use mde_workbench::cosmic_compat::IntoIcedColor;

/// Slide-out width (px) — a comfortable notification column.
const CENTER_WIDTH: f32 = 420.0;
/// Poll cadence — new alerts appear within this window of a bus publish.
const POLL_SECS: u64 = 8;
/// Cap on retained rows in the center (oldest dropped) — bounds a long uptime.
const MAX_ROWS: usize = 500;
/// LIGHTHOUSE-3 — the beam-animation tick cadence. The beam glyph itself comes
/// from the shared `lighthouse::beam_frame` (so the Hub footer and the Workbench
/// tab animate identically). The subscription is only armed when at least one
/// lighthouse is present, so an idle/empty Hub costs nothing.
const BEAM_TICK_MS: u64 = 150;

/// Single-instance guard — dep-free pidfile so re-launching the Action Center
/// (e.g. the applet bell pressed twice) never STACKS a second full-height
/// layer-surface. A live sibling → this launch exits; a stale/zombie holder →
/// this launch takes over. (Mirrors `single_instance.rs`, scoped to this bin.)
mod instance {
    use std::io::Write;
    use std::path::PathBuf;

    /// Outcome of the single-instance check.
    pub enum Primary {
        /// We own the lock — keep the handle alive for the process lifetime.
        Yes(Option<std::fs::File>),
        /// A live sibling already owns the panel — this launch must exit.
        No,
    }

    fn lock_path() -> PathBuf {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("mde-action-center.lock")
    }

    /// `true` if `pid` is a live (non-zombie) Action Center. The comm name is
    /// truncated to 15 chars by the kernel ("mde-notify-cent"); `starts_with`
    /// distinguishes it from the toast ("mde-notify-toas").
    fn live(pid: u32) -> bool {
        let comm_ok = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim().starts_with("mde-notify-c"))
            .unwrap_or(false);
        // A zombie (state Z after the parenthesized comm) is not a live primary.
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(')')
                    .and_then(|(_, a)| a.trim_start().chars().next())
            })
            .is_some_and(|st| st == 'Z');
        comm_ok && !zombie
    }

    /// Try to become the single primary. I/O failure degrades to running
    /// unprotected (`Yes(None)`) rather than refusing to start.
    pub fn acquire() -> Primary {
        let path = lock_path();
        if let Some(pid) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            if live(pid) {
                return Primary::No;
            }
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                let _ = f.flush();
                Primary::Yes(Some(f))
            }
            Err(_) => Primary::Yes(None),
        }
    }
}

fn main() -> Result<(), cosmic::iced::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // Single-instance: a live sibling already owns the panel — exit cleanly
    // instead of stacking another surface.
    let _lock = match instance::acquire() {
        instance::Primary::No => {
            tracing::info!("Action Center already running; exiting (no stacking).");
            return Ok(());
        }
        instance::Primary::Yes(handle) => handle,
    };
    cosmic::iced::daemon(|| (Center::new(), boot_task()), update, view)
        .title(namespace)
        .subscription(subscription)
        .theme(theme)
        .run()
}

fn namespace(_s: &Center, _id: window::Id) -> String {
    "mde-notify-center".to_string()
}

fn theme(_s: &Center, _id: window::Id) -> Theme {
    let p = Palette::dark();
    Theme::custom(
        "MDE Notification Hub".to_string(),
        cosmic::iced::theme::Palette {
            background: p.background.into_cosmic_color(),
            text: p.text.into_cosmic_color(),
            primary: p.accent.into_cosmic_color(),
            success: p.success.into_cosmic_color(),
            warning: p.warning.into_cosmic_color(),
            danger: p.danger.into_cosmic_color(),
        },
    )
}

/// NOTIFY-AC — the Now-Playing snapshot for the Music section.
#[derive(Debug, Clone, Default)]
struct MusicNow {
    active: bool,
    playing: bool,
    title: String,
    artist: String,
    /// AUDIT-MESH-4 — this peer has a working audio output device.
    audio_available: bool,
    /// AUDIT-MESH-4 — no Airsonic server configured yet (prompt to set one up).
    needs_airsonic: bool,
}

/// NOTIFY-AC — the voice agent snapshot for the Voice section.
#[derive(Debug, Clone, Default)]
struct VoiceStatus {
    registered: bool,
    listening: bool,
    detail: String,
    /// True when the snapshot's `ts` is within the staleness window (live agent).
    fresh: bool,
}

struct Center {
    items: Vec<AlertItem>,
    tail: AlertTail,
    /// Source-group labels the operator collapsed.
    collapsed: HashSet<String>,
    /// NOTIFY-AC — Now-Playing snapshot (`None` until first poll / music idle).
    music: Option<MusicNow>,
    /// NOTIFY-AC — voice agent snapshot (`None` when no agent has published).
    voice: Option<VoiceStatus>,
    /// AC-5 — live Do-Not-Disturb state (the quick-toggle tile reflects + flips
    /// the same `mde_bus::dnd` the toast/sound paths honor).
    dnd_active: bool,
    /// NOTIFY-UI-4 — in-flight handle for the shared-alerts read. The shared dir
    /// is on the QNM-Shared (FUSE) mount; reading it inline once hung the iced
    /// update loop on a wedged mount so the layer surface never mapped and the
    /// Action Center "wouldn't open". The read now runs on a helper thread and
    /// the result is picked up here non-blockingly — at most one read is ever in
    /// flight, so a permanently-wedged mount leaks a single thread, never the UI.
    shared_rx: Option<std::sync::mpsc::Receiver<Vec<AlertItem>>>,
    /// LIGHTHOUSE-3 — the pinned-footer beacons (one per `role==lighthouse`
    /// node), refreshed each poll from the replicated peer directory. Empty
    /// when no lighthouse is enrolled (the footer then hides itself).
    lighthouses: Vec<Beacon>,
    /// LIGHTHOUSE-3 — the beam-animation phase, advanced by the beam tick. Per-
    /// beacon position derives from this (healthy slow / unhealthy fast).
    beam_step: u16,
}

impl Center {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            tail: AlertTail::default(),
            collapsed: HashSet::new(),
            music: None,
            voice: None,
            dnd_active: false,
            shared_rx: None,
            lighthouses: Vec::new(),
            beam_step: 0,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    /// Periodic bus poll.
    Refresh,
    /// Collapse/expand a source group by its label.
    ToggleGroup(String),
    /// Acknowledge every alert.
    MarkAllRead,
    /// Drop every alert.
    ClearAll,
    /// Close the Action Center (X button / Esc / click-away). Exits the process
    /// so the single-instance lock is released and a later launch re-opens it.
    Close,
    /// Launch one of the bottom quick-launch apps and close the panel.
    OpenApp(&'static str),
    /// AC-5 — flip Do-Not-Disturb (quick-toggle tile).
    ToggleDnd,
    /// NOTIFY-AC — music transport: previous / play-pause toggle / next.
    MusicPrev,
    MusicToggle,
    MusicNext,
    /// LIGHTHOUSE-3 — advance the beacon beam animation one step.
    BeamTick,
    /// LIGHTHOUSE-3/4 — a lighthouse card was pressed: open the Workbench
    /// Lighthouses tab focused on this lighthouse (by hostname).
    OpenLighthouse(String),
}

/// NOTIFY-AC — fetch the Now-Playing snapshot over the bus (best-effort, short
/// timeouts). `None` when music is idle / the daemon is down.
fn fetch_music() -> Option<MusicNow> {
    use std::time::Duration;
    // get-state: flat transport reply {ok, playing, active, song_id, ...}.
    let state =
        mde_workbench::dbus::action_request("action/music/get-state", Duration::from_millis(700))?;
    let sv: serde_json::Value = serde_json::from_str(&state).ok()?;
    if sv.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Some(MusicNow::default());
    }
    // AUDIT-MESH-4 — capability flags so the section can show idle vs
    // needs-audio vs needs-Airsonic honestly (older daemons omit them →
    // assume capable/configured so we don't false-prompt a pre-fix peer).
    let audio_available = sv
        .get("audio_available")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let needs_airsonic = sv
        .get("needs_airsonic")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let active = sv
        .get("active")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let song_id = sv
        .get("song_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !active || song_id.is_empty() {
        return Some(MusicNow {
            audio_available,
            needs_airsonic,
            ..MusicNow::default()
        });
    }
    let playing = sv
        .get("playing")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    // get-song: browse reply {ok, result:{song:{title, artist}}}.
    let body = serde_json::json!({ "id": song_id }).to_string();
    let (title, artist) = mde_workbench::dbus::action_request_with_body(
        "action/music/get-song",
        Some(&body),
        Duration::from_millis(700),
    )
    .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
    .and_then(|v| {
        let song = v.get("result")?.get("song")?;
        let title = song
            .get("title")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let artist = song
            .get("artist")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Some((title, artist))
    })
    .unwrap_or_else(|| ("Unknown track".to_string(), String::new()));
    Some(MusicNow {
        active: true,
        playing,
        title,
        artist,
        audio_available,
        needs_airsonic,
    })
}

/// NOTIFY-AC — read the latest `state/voice/status` snapshot from the bus.
/// `fresh` is true when the agent's `ts` is within ~3× its heartbeat (live).
fn fetch_voice() -> Option<VoiceStatus> {
    let dir = mde_bus::client_data_dir()?;
    let persist = mde_bus::persist::Persist::open(dir).ok()?;
    // Newest message on the topic = the last row (list_since orders by ulid asc).
    let last = persist.list_since("state/voice/status", None).ok()?.pop()?;
    let body = last.body.as_deref().unwrap_or("");
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let now = (now_ms() / 1000).max(0) as u64;
    let ts = v.get("ts").and_then(serde_json::Value::as_u64).unwrap_or(0);
    Some(VoiceStatus {
        registered: v
            .get("registered")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        listening: v
            .get("listening")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        detail: v
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        fresh: now.saturating_sub(ts) <= 45,
    })
}

/// LIGHTHOUSE-3 — build the pinned-footer beacons from the replicated peer
/// directory (the same QNM-Shared roster the other panels read). One beacon per
/// `role==lighthouse` row, binary health per [`lighthouse::beacon_for`], with
/// the current lizardfs master (the leader-lease holder) held to the stricter
/// service check (Q3).
fn fetch_lighthouses() -> Vec<Beacon> {
    let root = mackes_mesh_types::peers::default_workgroup_root();
    let mut peers =
        mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(&root));
    // Back-fill role from the shell-status sidecar for records that predate the
    // role-stamping heartbeat (so the footer shows lighthouses before mackesd
    // rolls everywhere). Shared with the Workbench tab.
    mde_workbench::panels::lighthouses::enrich_roles(&root, &mut peers);
    let master = fetch_master_hostname(&root);
    let now = u64::try_from(now_ms()).unwrap_or(0);
    lighthouse::beacons(&peers, master.as_deref(), now, lighthouse::DEFAULT_STALE_MS)
}

/// LIGHTHOUSE-3 — the current lizardfs-master hostname, read best-effort from
/// the QNM leader lease (`<workgroup>/.mackesd-leader.lock`). The lease line is
/// `node_id\trenewed_at_s\tepoch`; the holder's `peer:<host>` node id maps to
/// `<host>`. Returns `None` when the lease is missing or expired (>60 s old),
/// in which case no lighthouse is treated as master (all use the lenient check).
fn fetch_master_hostname(workgroup_root: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(workgroup_root.join(".mackesd-leader.lock")).ok()?;
    let now_s = u64::try_from(now_ms() / 1000).unwrap_or(0);
    lighthouse::master_from_lease(&text, now_s)
}

fn subscription(s: &Center) -> Subscription<Message> {
    let poll = cosmic::iced::time::every(std::time::Duration::from_secs(POLL_SECS))
        .map(|_| Message::Refresh);
    // Esc closes the panel (W10 Action Center dismiss).
    let esc = cosmic::iced::event::listen_with(|event, _status, _window| {
        use cosmic::iced::keyboard::{key::Named, Event as Kbd, Key};
        if let cosmic::iced::Event::Keyboard(Kbd::KeyPressed { key, .. }) = event {
            if key == Key::Named(Named::Escape) {
                return Some(Message::Close);
            }
        }
        None
    });
    // LIGHTHOUSE-3 — only animate the beacons when at least one lighthouse is
    // shown; an empty/idle Hub footer costs no CPU (Q12 "inactive when hidden").
    let mut subs = vec![poll, esc];
    if !s.lighthouses.is_empty() {
        subs.push(
            cosmic::iced::time::every(std::time::Duration::from_millis(BEAM_TICK_MS))
                .map(|_| Message::BeamTick),
        );
    }
    Subscription::batch(subs)
}

/// Boot: spawn the right-anchored Overlay slide-out + first poll.
fn boot_task() -> Task<Message> {
    let id = window::Id::unique();
    Task::batch([
        get_layer_surface(SctkLayerSurfaceSettings {
            id,
            namespace: "mde-notify-center".to_string(),
            size: Some((Some(CENTER_WIDTH as u32), None)),
            exclusive_zone: CENTER_WIDTH as i32,
            anchor: Anchor::TOP.union(Anchor::BOTTOM).union(Anchor::RIGHT),
            layer: Layer::Overlay,
            // Interactive: its buttons need clicks + the surface takes focus
            // on demand (not a passive wallpaper).
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            ..Default::default()
        }),
        Task::done(Message::Refresh),
    ])
}

fn update(state: &mut Center, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => {
            // Poll the live system bus synchronously (a quick SQLite read;
            // Persist is !Send so it's opened + dropped within this call,
            // never held across an await).
            if let Some(dir) = mde_bus::client_data_dir() {
                if let Ok(persist) = mde_bus::persist::Persist::open(dir) {
                    let fresh = state.tail.poll(&persist);
                    // Newest first; cap the retained set.
                    for item in fresh {
                        state.items.insert(0, item);
                    }
                    state.items.truncate(MAX_ROWS);
                }
            }
            // NOTIFY-DIST-2 — also read the replicated shared-alerts dir so the
            // panel shows mesh-wide notifications (every peer's mirrored alerts),
            // not just this node's. Deduped against the local tail (shared
            // dedup set). The workgroup root honors MDE_WORKGROUP_ROOT.
            //
            // NOTIFY-UI-4 — this dir is on the QNM-Shared (FUSE) mount, so the
            // read MUST NOT run inline: a wedged mount blocks uninterruptibly,
            // which previously froze the iced update loop on the very first
            // Refresh (batched with the layer-surface creation in boot_task) so
            // the surface never mapped and the panel "wouldn't open". Instead, run
            // the read on a helper thread and pick the result up non-blockingly:
            //   * if a prior read finished, dedup + merge it now;
            //   * if none is in flight, kick a new one off;
            //   * if one is still running (slow/wedged mount), do nothing this
            //     cycle — the UI stays fully responsive, at most one read is ever
            //     in flight, and a recovered mount is picked up on a later poll.
            {
                if let Some(rx) = &state.shared_rx {
                    match rx.try_recv() {
                        Ok(items) => {
                            state.shared_rx = None;
                            let fresh = state.tail.dedup_fresh(items);
                            for item in fresh {
                                state.items.insert(0, item);
                            }
                            state.items.sort_by(|a, b| b.ts_unix_ms.cmp(&a.ts_unix_ms));
                            state.items.truncate(MAX_ROWS);
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            state.shared_rx = None;
                        }
                    }
                }
                if state.shared_rx.is_none() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let wg = mackes_mesh_types::peers::default_workgroup_root();
                        let _ = tx.send(mde_notify::read_shared_alert_items(&wg));
                    });
                    state.shared_rx = Some(rx);
                }
            }
            // NOTIFY-AC — refresh the Music + Voice section snapshots.
            state.music = fetch_music();
            state.voice = fetch_voice();
            // AC-5 — reflect the live DND state in the quick-toggle.
            if let Some(dir) = mde_bus::client_data_dir() {
                state.dnd_active = mde_bus::dnd::load_default(&dir).active;
            }
            // LIGHTHOUSE-3 — refresh the pinned lighthouse footer beacons.
            state.lighthouses = fetch_lighthouses();
        }
        Message::BeamTick => {
            state.beam_step = state.beam_step.wrapping_add(1);
        }
        Message::OpenLighthouse(host) => {
            // LIGHTHOUSE-3/4 — deep-link to the Workbench Lighthouses tab,
            // focused on this lighthouse. `mde-workbench` is single-instance:
            // a running primary picks up the focus over the Bus, otherwise this
            // launch becomes the primary and opens at the panel (spawn-if-
            // needed, no duplicate window). Then close the Hub.
            let focus = format!("mesh.lighthouses:{host}");
            let _ = std::process::Command::new("mde-workbench")
                .args(["--focus", &focus])
                .spawn();
            std::process::exit(0);
        }
        Message::ToggleDnd => {
            // AC-5 — flip + persist DND to the same store the toast/sound paths
            // read, so the quick-toggle is authoritative, not a separate flag.
            if let Some(dir) = mde_bus::client_data_dir() {
                let mut st = mde_bus::dnd::load_default(&dir);
                st.active = !st.active;
                if mde_bus::dnd::save_default(&dir, &st).is_ok() {
                    state.dnd_active = st.active;
                }
            }
        }
        Message::MusicPrev | Message::MusicToggle | Message::MusicNext => {
            use std::time::Duration;
            // Resume vs pause depends on the live state; default to resume.
            let verb = match message {
                Message::MusicPrev => "action/music/prev",
                Message::MusicNext => "action/music/next",
                _ => {
                    if state.music.as_ref().is_some_and(|m| m.playing) {
                        "action/music/pause"
                    } else {
                        "action/music/resume"
                    }
                }
            };
            let _ = mde_workbench::dbus::action_request(verb, Duration::from_millis(700));
            // Reflect the new transport state immediately.
            state.music = fetch_music();
        }
        Message::ToggleGroup(label) => {
            if !state.collapsed.remove(&label) {
                state.collapsed.insert(label);
            }
        }
        Message::MarkAllRead => {
            for it in &mut state.items {
                it.read = true;
            }
        }
        Message::ClearAll => state.items.clear(),
        Message::OpenApp(cmd) => {
            // Spawn the target app (detached) then close the panel.
            let _ = std::process::Command::new(cmd).spawn();
            std::process::exit(0);
        }
        Message::Close => {
            // Exit so the single-instance lock is released; the applet bell (or
            // any launch) re-opens a fresh panel. A layer-shell daemon has no
            // window to "hide", so closing == exiting.
            std::process::exit(0);
        }
    }
    Task::none()
}

/// Source render order (stable group ordering, matching the design).
fn source_rank(s: &Source) -> u8 {
    match s {
        Source::Security => 0,
        Source::Firewall => 1,
        Source::Presence => 2,
        Source::Compute => 3,
        Source::Peer(_) => 4,
        Source::DesktopApp => 5,
        Source::System => 6,
    }
}

/// Group items by source (stable order), items within a group newest-first.
/// Pure + testable.
#[must_use]
pub fn group_items(items: &[AlertItem]) -> Vec<(Source, Vec<AlertItem>)> {
    let mut groups: Vec<(Source, Vec<AlertItem>)> = Vec::new();
    for it in items {
        if let Some(g) = groups.iter_mut().find(|(s, _)| *s == it.source) {
            g.1.push(it.clone());
        } else {
            groups.push((it.source.clone(), vec![it.clone()]));
        }
    }
    for (_, v) in &mut groups {
        v.sort_by(|a, b| b.ts_unix_ms.cmp(&a.ts_unix_ms));
    }
    groups.sort_by_key(|(s, _)| source_rank(s));
    groups
}

/// The highest (most-severe) severity in a group — drives the group accent.
#[must_use]
pub fn group_severity(items: &[AlertItem]) -> Severity {
    items
        .iter()
        .map(|i| i.severity)
        .min()
        .unwrap_or(Severity::Info)
}

/// One-glyph severity marker.
fn severity_glyph(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "●",
        Severity::Warning => "◐",
        Severity::Info => "○",
        Severity::Success => "✓",
    }
}

/// Compact "Nm ago" age. Pure + testable.
#[must_use]
pub fn format_age(ts_unix_ms: i64, now_unix_ms: i64) -> String {
    let secs = ((now_unix_ms - ts_unix_ms) / 1000).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn view(state: &Center, _id: window::Id) -> Element<'_, Message> {
    let p = Palette::dark();
    let now = now_ms();

    // Header: title + close on the top line, the bulk actions on their own
    // line below so a long "· N unread" title never collides with the buttons
    // (the panel is only ~390px wide). Generous top/side padding so nothing is
    // jammed against the window edge.
    let unread = state.items.iter().filter(|i| !i.read).count();
    let title = text(format!("Notification Hub · {unread} unread"))
        .size(16)
        .color(p.text.into_cosmic_color());
    let title_row = row![
        title,
        Space::new().width(Length::Fill),
        // Close (✕) — also bound to Esc + click-away.
        action_button("✕", Message::Close, p),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    let actions = row![
        action_button("Mark all read", Message::MarkAllRead, p),
        Space::new().width(Length::Fixed(8.0)),
        action_button("Clear all", Message::ClearAll, p),
    ];
    let header = column![title_row, actions].spacing(10);

    let mut body = column![header, Space::new().height(Length::Fixed(12.0))]
        .spacing(8)
        .padding(Padding::from([14u16, 16u16]));

    if state.items.is_empty() {
        body = body.push(
            text("No alerts.")
                .size(13)
                .color(p.text_muted.into_cosmic_color()),
        );
    } else {
        for (source, group) in group_items(&state.items) {
            let label = source.label();
            let accent = severity_token(group_severity(&group), &p).into_cosmic_color();
            let collapsed = state.collapsed.contains(&label);
            let caret = if collapsed { "▸" } else { "▾" };
            // Group header — clickable to toggle.
            let head = button(
                row![
                    text(caret).size(12).color(p.text_muted.into_cosmic_color()),
                    Space::new().width(Length::Fixed(6.0)),
                    text(format!("{label} ({})", group.len()))
                        .size(13)
                        .color(p.text.into_cosmic_color()),
                    Space::new().width(Length::Fill),
                    text(severity_glyph(group_severity(&group)))
                        .size(13)
                        .color(accent),
                ]
                .align_y(cosmic::iced::Alignment::Center),
            )
            .on_press(Message::ToggleGroup(label.clone()))
            .width(Length::Fill);
            body = body.push(head);
            if !collapsed {
                for item in &group {
                    body = body.push(alert_row(item, now, p));
                }
            }
        }
    }

    let scroll = scrollable(
        container(body)
            .padding(Padding::from([12u16, 14u16]))
            .width(Length::Fill),
    );

    // Bottom quick-launch bar (W10 Action Center "quick actions" row): open the
    // Workbench, MDE-Files, or Cosmic Settings, then dismiss the panel.
    let launch_bar = container(
        row![
            launch_tile("Workbench", "mde-workbench", p),
            launch_tile("MDE-Files", "mde-files", p),
            launch_tile("Settings", "cosmic-settings", p),
        ]
        .spacing(8),
    )
    .padding(Padding::from([10u16, 14u16]))
    .width(Length::Fill);

    // AC-5 — W10-style quick-actions row (toggles), above the app launchers.
    let dnd_label = if state.dnd_active {
        "\u{25CB} Do Not Disturb · on"
    } else {
        "\u{25CF} Do Not Disturb · off"
    };
    let quick_actions = container(
        row![quick_toggle(
            dnd_label,
            state.dnd_active,
            Message::ToggleDnd,
            p
        )]
        .spacing(8),
    )
    .padding(Padding::from([6u16, 14u16]))
    .width(Length::Fill);

    // Pinned bottom sections, top-to-bottom: Now-Playing, Voice/SIP, then the
    // LIGHTHOUSE-3 lighthouses footer (only when lighthouses exist), then the
    // quick-actions + app launchers. Built as a vec so the footer + its divider
    // appear only when there is something to show.
    let mut sections: Vec<Element<'_, Message>> = vec![
        container(scroll).height(Length::Fill).into(),
        section_divider(p),
        now_playing_section(state.music.as_ref(), p),
        section_divider(p),
        voice_section(state.voice.as_ref(), p),
    ];
    if !state.lighthouses.is_empty() {
        sections.push(section_divider(p));
        sections.push(lighthouses_footer(&state.lighthouses, state.beam_step, p));
    }
    sections.push(section_divider(p));
    sections.push(quick_actions.into());
    sections.push(launch_bar.into());

    container(cosmic::iced::widget::column(sections).spacing(0))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A thin horizontal divider between the pinned bottom sections.
fn section_divider(p: Palette) -> Element<'static, Message> {
    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
        .style(move |_| container::Style {
            snap: false,
            background: Some(cosmic::iced::Background::Color(
                p.border.into_cosmic_color(),
            )),
            ..Default::default()
        })
        .into()
}

/// NOTIFY-AC — the "Now Playing" media section: track line + transport controls.
/// Honest idle state when nothing is playing / the music daemon is down.
fn now_playing_section(music: Option<&MusicNow>, p: Palette) -> Element<'static, Message> {
    let body: Element<'static, Message> = match music {
        Some(m) if m.active => {
            let track = if m.artist.is_empty() {
                m.title.clone()
            } else {
                format!("{} — {}", m.title, m.artist)
            };
            let toggle_glyph = if m.playing { "\u{2016}" } else { "\u{25B6}" };
            row![
                text("♪").size(14).color(p.accent.into_cosmic_color()),
                Space::new().width(Length::Fixed(8.0)),
                text(track)
                    .size(12)
                    .color(p.text.into_cosmic_color())
                    .width(Length::Fill),
                transport_button("\u{25C0}", Message::MusicPrev, p),
                transport_button(toggle_glyph, Message::MusicToggle, p),
                transport_button("\u{25B6}", Message::MusicNext, p),
            ]
            .align_y(cosmic::iced::Alignment::Center)
            .into()
        }
        // AUDIT-MESH-4 — honest idle states instead of a silent blank: tell
        // the operator when no Airsonic server is configured or this peer has
        // no audio device, otherwise plain "Nothing playing".
        Some(m) if m.needs_airsonic => row![
            text("♪").size(14).color(p.text_muted.into_cosmic_color()),
            Space::new().width(Length::Fixed(8.0)),
            text("Configure an Airsonic server to play music")
                .size(12)
                .color(p.text.into_cosmic_color())
                .width(Length::Fill),
            action_button("Set up", Message::OpenApp("mde-workbench"), p),
        ]
        .align_y(cosmic::iced::Alignment::Center)
        .into(),
        Some(m) if !m.audio_available => text("♪  No audio device on this peer")
            .size(12)
            .color(p.text_muted.into_cosmic_color())
            .into(),
        _ => text("♪  Nothing playing")
            .size(12)
            .color(p.text_muted.into_cosmic_color())
            .into(),
    };
    container(body)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .into()
}

/// NOTIFY-AC — the Voice section: agent registration + listening state.
fn voice_section(voice: Option<&VoiceStatus>, p: Palette) -> Element<'static, Message> {
    let (glyph, gcolor, line) = match voice {
        Some(v) if v.fresh => {
            let g = if v.listening { "●" } else { "○" };
            let c = if v.listening {
                p.success
            } else if v.registered {
                p.accent
            } else {
                p.text_muted
            };
            let detail = if v.detail.is_empty() {
                if v.registered {
                    "registered".to_string()
                } else {
                    "not registered".to_string()
                }
            } else {
                v.detail.clone()
            };
            let listen = if v.listening { " · listening" } else { "" };
            (g, c, format!("Voice · {detail}{listen}"))
        }
        _ => ("○", p.text_muted, "Voice · agent offline".to_string()),
    };
    container(
        row![
            text(glyph).size(13).color(gcolor.into_cosmic_color()),
            Space::new().width(Length::Fixed(8.0)),
            text(line)
                .size(12)
                .color(p.text.into_cosmic_color())
                .width(Length::Fill),
            action_button("Open", Message::OpenApp("mde-voice-hud"), p),
        ]
        .align_y(cosmic::iced::Alignment::Center),
    )
    .padding(Padding::from([10u16, 14u16]))
    .width(Length::Fill)
    .into()
}

/// A compact transport control button.
/// LIGHTHOUSE-3 — the pinned Lighthouses footer (Q5): a header (beacon glyph +
/// "Lighthouses" + live `N/M healthy`, Q8) over a horizontally-scrollable strip
/// of square beacon cards (Q6/Q7). Colors come only from `mde-theme` tokens
/// (`beacon_healthy` / `danger`, §4).
fn lighthouses_footer(beacons: &[Beacon], beam_step: u16, p: Palette) -> Element<'static, Message> {
    let (healthy, total) = lighthouse::health_counts(beacons);
    let count_color = if healthy == total {
        p.beacon_healthy
    } else {
        p.danger
    };
    let header = row![
        text("\u{25C9}") // ◉ fisheye — an abstract light source (BMP, not emoji)
            .size(13)
            .color(count_color.into_cosmic_color()),
        Space::new().width(Length::Fixed(8.0)),
        text("Lighthouses")
            .size(13)
            .color(p.text.into_cosmic_color())
            .width(Length::Fill),
        text(format!("{healthy}/{total} healthy"))
            .size(12)
            .color(count_color.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::Alignment::Center);

    let cards: Vec<Element<'static, Message>> = beacons
        .iter()
        .map(|b| beacon_card(b, beam_step, p))
        .collect();
    let strip = scrollable(row(cards).spacing(10)).direction(
        cosmic::iced::widget::scrollable::Direction::Horizontal(
            cosmic::iced::widget::scrollable::Scrollbar::new().width(4).scroller_width(4),
        ),
    );

    container(
        column![
            header,
            Space::new().height(Length::Fixed(8.0)),
            strip,
        ]
        .spacing(0),
    )
    .padding(Padding::from([10u16, 14u16]))
    .width(Length::Fill)
    .into()
}

/// LIGHTHOUSE-3 — one square beacon card: the animated beam square (border in
/// the status color) over name / overlay IP / status word (Q16). The whole card
/// presses through to the Workbench Lighthouses tab (Q19).
fn beacon_card(b: &Beacon, beam_step: u16, p: Palette) -> Element<'static, Message> {
    let healthy = b.healthy();
    let color = if healthy { p.beacon_healthy } else { p.danger };
    let glyph = lighthouse::beam_frame(healthy, beam_step);
    let square = container(text(glyph).size(22).color(color.into_cosmic_color()))
        .center_x(Length::Fixed(54.0))
        .center_y(Length::Fixed(54.0))
        .style(move |_| container::Style {
            background: Some(cosmic::iced::Background::Color(p.surface.into_cosmic_color())),
            border: cosmic::iced::Border {
                color: color.into_cosmic_color(),
                width: 2.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        });
    let ip = b.overlay_ip.clone().unwrap_or_else(|| "—".to_string());
    let body = column![
        square,
        text(b.hostname.clone())
            .size(11)
            .color(p.text.into_cosmic_color()),
        text(ip).size(10).color(p.text_muted.into_cosmic_color()),
        text(b.status.word())
            .size(10)
            .color(color.into_cosmic_color()),
    ]
    .spacing(2)
    .align_x(cosmic::iced::Alignment::Center)
    .width(Length::Fixed(78.0));
    button(body)
        .on_press(Message::OpenLighthouse(b.hostname.clone()))
        .style(|_, _| button::Style {
            background: None,
            ..Default::default()
        })
        .padding(0)
        .into()
}

fn transport_button(glyph: &str, msg: Message, p: Palette) -> Element<'_, Message> {
    button(
        text(glyph.to_string())
            .size(14)
            .color(p.text.into_cosmic_color()),
    )
    .padding(Padding::from([4u16, 8u16]))
    .on_press(msg)
    .into()
}

/// A bottom quick-launch tile: label + the binary it spawns (`OpenApp`).
fn launch_tile<'a>(label: &'a str, cmd: &'static str, p: Palette) -> Element<'a, Message> {
    button(
        container(text(label).size(12).color(p.text.into_cosmic_color()))
            .center_x(Length::Fill)
            .padding(Padding::from([8u16, 6u16])),
    )
    .width(Length::Fill)
    .on_press(Message::OpenApp(cmd))
    .into()
}

/// AC-5 — a W10-style quick-action toggle tile. When `on`, the label is drawn in
/// the Carbon accent so the active state reads at a glance (matching the W10
/// Action Center's highlighted quick-actions).
fn quick_toggle<'a>(label: &'a str, on: bool, msg: Message, p: Palette) -> Element<'a, Message> {
    let fg = if on { p.accent } else { p.text_muted };
    button(
        container(text(label).size(12).color(fg.into_cosmic_color()))
            .center_x(Length::Fill)
            .padding(Padding::from([8u16, 6u16])),
    )
    .width(Length::Fill)
    .on_press(msg)
    .into()
}

/// One alert row: severity glyph (colored) · age · host · title / body. Takes the
/// item by value so the returned element owns its text (no borrow of the caller's
/// loop-local group).
fn alert_row(item: &AlertItem, now_ms: i64, p: Palette) -> Element<'static, Message> {
    let sev_color = severity_token(item.severity, &p).into_cosmic_color();
    let title_color = if item.read { p.text_muted } else { p.text }.into_cosmic_color();
    let host = item.host.clone().unwrap_or_default();
    let meta = if host.is_empty() {
        format_age(item.ts_unix_ms, now_ms)
    } else {
        format!("{} · {host}", format_age(item.ts_unix_ms, now_ms))
    };
    let head = row![
        text(severity_glyph(item.severity))
            .size(13)
            .color(sev_color),
        Space::new().width(Length::Fixed(8.0)),
        text(item.title.clone()).size(13).color(title_color),
        Space::new().width(Length::Fill),
        text(meta).size(11).color(p.text_muted.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    let mut col = column![head].spacing(2);
    if !item.body.is_empty() {
        let body: String = item.body.chars().take(200).collect();
        col = col.push(text(body).size(11).color(p.text_muted.into_cosmic_color()));
    }
    container(col)
        .padding(Padding::from([6u16, 8u16]))
        .width(Length::Fill)
        .into()
}

fn action_button<'a>(label: &'a str, msg: Message, p: Palette) -> Element<'a, Message> {
    button(text(label).size(12).color(p.text.into_cosmic_color()))
        .padding(Padding::from([4u16, 10u16]))
        .on_press(msg)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, src: Source, sev: Severity, ts: i64) -> AlertItem {
        AlertItem {
            id: id.into(),
            ts_unix_ms: ts,
            severity: sev,
            source: src,
            topic: "t".into(),
            host: None,
            title: "x".into(),
            body: String::new(),
            read: false,
        }
    }

    #[test]
    fn group_items_orders_groups_and_sorts_newest_first() {
        let items = vec![
            item("a", Source::System, Severity::Info, 10),
            item("b", Source::Security, Severity::Critical, 20),
            item("c", Source::Security, Severity::Warning, 30),
        ];
        let groups = group_items(&items);
        // Security ranks before System.
        assert_eq!(groups[0].0, Source::Security);
        assert_eq!(groups[1].0, Source::System);
        // Within Security, newest (ts 30) first.
        assert_eq!(groups[0].1[0].id, "c");
        assert_eq!(groups[0].1[1].id, "b");
    }

    #[test]
    fn group_severity_is_the_most_severe() {
        let g = vec![
            item("a", Source::Security, Severity::Info, 1),
            item("b", Source::Security, Severity::Critical, 2),
            item("c", Source::Security, Severity::Warning, 3),
        ];
        assert_eq!(group_severity(&g), Severity::Critical);
    }

    #[test]
    fn format_age_buckets() {
        assert_eq!(format_age(0, 5_000), "5s");
        assert_eq!(format_age(0, 120_000), "2m");
        assert_eq!(format_age(0, 7_200_000), "2h");
        assert_eq!(format_age(0, 172_800_000), "2d");
        // Clock skew (future ts) clamps to 0s, never negative.
        assert_eq!(format_age(10_000, 0), "0s");
    }
}
