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

mod motion;
mod notify_clipboard;
use notify_clipboard::ClipRow;

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    get_layer_surface, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{window, Element, Length, Padding, Subscription, Task, Theme};
use mackes_mesh_types::lighthouse::{self, Beacon};
use mde_notify::{severity_token, AlertItem, AlertTail, Severity, Source};
use mde_theme::Palette;

use motion::{collapse_stacks, HubAnim, Stack};
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
/// NOTIFY-HUB-2 — the new-item slide/blink animation tick cadence (~60 fps). The
/// subscription is only armed while [`HubAnim::is_idle`] is `false`, so an idle
/// Hub (no entrance in flight) costs nothing.
const ANIM_TICK_MS: u64 = 16;

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

/// NOTIFY-HUB-1 — resolve the live palette from the user's MDE theme preference
/// so the Hub honors light + dark (Carbon Gray 10 / 90 / 100), matching the
/// Application Menu (which is already theme-aware) instead of a hardcoded dark.
fn hub_palette() -> Palette {
    Palette::for_theme(mde_theme::Preferences::load().theme)
}

fn theme(_s: &Center, _id: window::Id) -> Theme {
    let p = hub_palette();
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
    /// MUSIC-HUB-2 — the current track's `coverArt` token (for the art thumbnail).
    cover_art: Option<String>,
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
    /// MUSIC-HUB-2 — decoded cover art for the current track + the coverArt id it
    /// was fetched for (so the art is re-fetched only when the track changes).
    now_art: Option<cosmic::iced::widget::image::Handle>,
    now_art_id: Option<String>,
    /// NOTIFY-HUB-2 — the new-item motion state machine (slide-in + 2× severity
    /// blink for fresh cards; slide-down for the items they push). Advanced by the
    /// animation tick, which is only armed while it has an entrance in flight.
    anim: HubAnim,
    /// NOTIFY-HUB-2 — every item id ever surfaced, so a `Refresh` can tell which
    /// rows are *new* (and thus animate in) vs. already on screen.
    seen_ids: HashSet<String>,
    /// NOTIFY-HUB-2 — collapsed same-source stacks the operator has expanded (by
    /// [`Stack::key`]); a stack not in here renders folded with its count badge.
    expanded_stacks: HashSet<String>,
    /// CLIP-VIEW-1 — the mesh-global clipboard history (newest first), refreshed
    /// each poll from `action/clipboard/list`. Empty until the first list reply /
    /// when the daemon is down. Each row renders as text + source-node + age, is
    /// click-to-copy onto THIS node, and carries per-entry pin + delete.
    clips: Vec<ClipRow>,
    /// NOTIFY-3 (L656) — persisted read/cleared cursors. Mark-all-read + clear-all
    /// advance these and write `<bus_root>/notify-read.yaml`, so the acknowledgement
    /// + dismissal survive a restart (otherwise the bus tail re-reads the same
    /// backlog as unread + un-cleared on every relaunch). Applied after each poll
    /// merge by [`Center::apply_read_state`].
    read_state: mde_notify::ReadState,
}

impl Center {
    fn new() -> Self {
        let reduce_motion = mde_theme::Preferences::load().a11y.reduce_motion;
        // NOTIFY-3 (L656) — load the persisted read/cleared cursors so the panel
        // opens honoring the operator's last mark-all-read / clear-all (no Bus
        // data dir → empty cursors, i.e. the whole history is visible + unread).
        let read_state = mde_bus::client_data_dir()
            .map(|d| mde_notify::ReadState::load(&d))
            .unwrap_or_default();
        // NOTIFY-FX-1 — arm the Hub-open reveal as the panel mounts, so it
        // fades/slides in with the same Carbon panel-mount vocabulary the
        // Application Menu plays on open (the launcher's idiom).
        let mut anim = HubAnim::new(reduce_motion);
        anim.on_open(std::time::Instant::now());
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
            now_art: None,
            now_art_id: None,
            anim,
            seen_ids: HashSet::new(),
            expanded_stacks: HashSet::new(),
            clips: Vec::new(),
            read_state,
        }
    }

    /// NOTIFY-3 (L656) — apply the persisted cursors to the current rows: drop any
    /// alert at/below the cleared cursor and flag any at/below the read cursor as
    /// acknowledged. Run after every poll merge so a relaunch (which re-reads the
    /// backlog off the bus tail + shared dir) reflects the operator's last
    /// clear-all / mark-all-read instead of resurrecting the whole history.
    fn apply_read_state(&mut self) {
        let rs = &self.read_state;
        self.items.retain(|it| !rs.is_cleared(&it.id));
        for it in &mut self.items {
            if self.read_state.is_read(&it.id) {
                it.read = true;
            }
        }
    }

    /// NOTIFY-3 (L656) — persist the read/cleared cursors next to the bus data
    /// (best-effort; a write failure leaves the in-memory state intact and is
    /// retried on the next mark-all-read / clear-all).
    fn persist_read_state(&self) {
        if let Some(dir) = mde_bus::client_data_dir() {
            let _ = self.read_state.save(&dir);
        }
    }

    /// NOTIFY-HUB-2 — register any rows that are new since the last frame so they
    /// animate in (slide + blink). On the very first poll the panel is just
    /// opening, so the whole initial set is treated as already-present (no mass
    /// strobe on open) — only adds *after* the first frame animate.
    ///
    /// `seen_ids` is afterward pruned to the ids still present in `items` so it
    /// stays bounded by `MAX_ROWS` over a long uptime (no unbounded growth), and
    /// so an id that was evicted by the `truncate` cap and later re-arrives
    /// correctly animates in again as a fresh card.
    fn note_new_items(&mut self, now: std::time::Instant) {
        let first_frame = self.seen_ids.is_empty();
        let fresh: Vec<String> = self
            .items
            .iter()
            .filter(|i| !self.seen_ids.contains(&i.id))
            .map(|i| i.id.clone())
            .collect();
        // Re-seed the seen-set to exactly the live rows (drops evicted ids).
        self.seen_ids = self.items.iter().map(|i| i.id.clone()).collect();
        if !first_frame && !fresh.is_empty() {
            self.anim.on_new_items(fresh, now);
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    /// Periodic bus poll. NOTIFY-RENDER-LAG-1 — this arm now ONLY dispatches the
    /// off-thread fetch Tasks (and merges the cheap local shared-alert tail); the
    /// blocking bus round-trips run on `spawn_blocking` workers and land back as
    /// the `*Loaded` variants below, so the layer surface maps + paints its chrome
    /// immediately instead of waiting on the ~5.3 s cumulative timeout budget.
    Refresh,
    /// NOTIFY-RENDER-LAG-1 — the Now-Playing snapshot finished loading off-thread.
    MusicLoaded(Option<MusicNow>),
    /// NOTIFY-RENDER-LAG-1 — the voice-agent snapshot finished loading off-thread.
    VoiceLoaded(Option<VoiceStatus>),
    /// NOTIFY-RENDER-LAG-1 — the clipboard history finished loading off-thread.
    ClipsLoaded(Vec<ClipRow>),
    /// NOTIFY-RENDER-LAG-1 — the live DND state finished loading off-thread.
    /// `None` (no Bus data dir) leaves the current toggle unchanged.
    DndLoaded(Option<bool>),
    /// NOTIFY-RENDER-LAG-1 — the lighthouse footer beacons finished loading
    /// off-thread.
    LighthousesLoaded(Vec<Beacon>),
    /// NOTIFY-RENDER-LAG-1 — cover art for `cover_id` finished decoding off-thread
    /// (gated: only fetched AFTER `MusicLoaded`, never on the first frame).
    CoverArtLoaded(String, Option<cosmic::iced::widget::image::Handle>),
    /// Collapse/expand a source group by its label.
    ToggleGroup(String),
    /// NOTIFY-HUB-2 — expand/collapse a same-source stack by its [`Stack::key`].
    ToggleStack(String),
    /// NOTIFY-HUB-2 — advance the new-item slide/blink animation one frame.
    AnimTick,
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
    /// CLIP-VIEW-1 — load a clip row's text into THIS node's Wayland clipboard
    /// (`wl-copy`). The carried text is the verbatim clip, not the preview.
    ClipCopy(String),
    /// CLIP-VIEW-1 — toggle an entry's pin mesh-wide (`action/clipboard/{pin,
    /// unpin}`). Carries the entry id + its current pinned state.
    ClipTogglePin(String, bool),
    /// CLIP-VIEW-1 — drop one entry mesh-wide (`action/clipboard/delete`).
    ClipDelete(String),
    /// CLIP-VIEW-1 — clear every UNPINNED entry mesh-wide; pinned survive
    /// (`action/clipboard/clear`).
    ClipClearAll,
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
    let (title, artist, cover_art) = mde_workbench::dbus::action_request_with_body(
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
        // MUSIC-HUB-2 — the coverArt token for the art thumbnail.
        let cover_art = song
            .get("coverArt")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        Some((title, artist, cover_art))
    })
    .unwrap_or_else(|| ("Unknown track".to_string(), String::new(), None));
    Some(MusicNow {
        active: true,
        playing,
        title,
        artist,
        audio_available,
        needs_airsonic,
        cover_art,
    })
}

/// MUSIC-HUB-2 — fetch + decode the current track's cover art over the Bus
/// (`action/music/get-cover-art` with the coverArt token in the body → base64
/// `result.art`). `None` when there's no art / the daemon is down.
fn fetch_cover_art(cover_id: &str) -> Option<cosmic::iced::widget::image::Handle> {
    use base64::Engine;
    use std::time::Duration;
    let reply = mde_workbench::dbus::action_request_with_body(
        "action/music/get-cover-art",
        Some(cover_id),
        Duration::from_millis(1200),
    )?;
    let v: serde_json::Value = serde_json::from_str(&reply).ok()?;
    let b64 = v.get("result")?.get("art")?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(cosmic::iced::widget::image::Handle::from_bytes(bytes))
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

/// CLIP-VIEW-1 — list the mesh-global clipboard history over the Bus
/// (`action/clipboard/list`, CLIP-SYNC-1). Best-effort with a short timeout;
/// an empty Vec on no daemon / no reply / a decode failure so the section
/// shows its honest empty state instead of stalling the poll.
fn fetch_clips() -> Vec<ClipRow> {
    use std::time::Duration;
    match mde_workbench::dbus::action_request("action/clipboard/list", Duration::from_millis(700)) {
        Some(reply) => notify_clipboard::parse_list_reply(&reply),
        None => Vec::new(),
    }
}

/// CLIP-VIEW-1 — fire one `action/clipboard/{verb}` mutation with `body` (an
/// entry id, or `None` for `clear`) and return the freshly-listed history so
/// the section reflects the mesh-wide edit immediately. The verbs hit the same
/// shared `clipboard/history.json` the capture worker appends to, so the change
/// is mesh-wide; we re-list rather than mutate `state.clips` locally so the
/// rendered set always matches the authoritative shared document.
fn clip_mutate(verb: &str, body: Option<&str>) -> Vec<ClipRow> {
    use std::time::Duration;
    let topic = format!("action/clipboard/{verb}");
    let _ = mde_workbench::dbus::action_request_with_body(&topic, body, Duration::from_millis(700));
    fetch_clips()
}

/// CLIP-VIEW-1 — load `text` onto THIS node's Wayland clipboard via `wl-copy`,
/// piping over stdin (no shell-quoting hazard) and detaching. Best-effort: a
/// host without `wl-clipboard` / no Wayland session simply no-ops. The capture
/// worker's O2 debounce drops the resulting `wl-paste --watch` echo so a
/// click-to-load never duplicates the entry it loaded. Mirrors the daemon's
/// `kdc_host::apply_clipboard` idiom.
fn load_into_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return; // wl-copy absent / no Wayland — skip cleanly.
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    // Don't block the update loop waiting on wl-copy; it exits promptly.
    drop(child);
}

/// LIGHTHOUSE-3 — build the pinned-footer beacons from the replicated peer
/// directory (the same QNM-Shared roster the other panels read). One beacon per
/// `role==lighthouse` row, binary health per [`lighthouse::beacon_for`], with
/// the current lizardfs master (the leader-lease holder) held to the stricter
/// service check (Q3).
fn fetch_lighthouses() -> Vec<Beacon> {
    let root = mackes_mesh_types::peers::default_workgroup_root();
    // SUBSTRATE-8 — peers + leader from the directory RPC (etcd-or-fs), not a
    // direct roster glob + `.mackesd-leader.lock` read, so the footer survives
    // the substrate cutover.
    let (mut peers, master) = mde_workbench::mesh_directory::fetch_peers_and_leader();
    // Back-fill role from the shell-status sidecar for records that predate the
    // role-stamping heartbeat (so the footer shows lighthouses before mackesd
    // rolls everywhere). Shared with the Workbench tab.
    mde_workbench::panels::lighthouses::enrich_roles(&root, &mut peers);
    let now = u64::try_from(now_ms()).unwrap_or(0);
    lighthouse::beacons(&peers, master.as_deref(), now, lighthouse::DEFAULT_STALE_MS)
}

/// AC-5 — read the live Do-Not-Disturb `active` flag. Split out as a named fn so
/// the off-thread [`refresh_tasks`] dispatch can `spawn_blocking` it: the DND
/// store lives on the GFS-replicated `mesh-home`, so (like the shared-alerts
/// read, NOTIFY-UI-4) a wedged mount must never block the iced update loop.
///
/// `None` when there's no Bus data dir — the caller then leaves the live toggle
/// UNCHANGED rather than force-clearing it (preserving the pre-NOTIFY-RENDER-LAG-1
/// inline behavior, which only assigned `dnd_active` inside `if let Some(dir)`).
fn fetch_dnd() -> Option<bool> {
    let dir = mde_bus::client_data_dir()?;
    Some(mde_bus::dnd::load_default(&dir).active)
}

/// NOTIFY-RENDER-LAG-1 — the off-thread fetch dispatch the `Refresh` arm fires.
///
/// Each blocking bus round-trip (`fetch_music`/`fetch_voice`/`fetch_clips`/
/// `fetch_dnd`/`fetch_lighthouses`) runs on its own `spawn_blocking` worker and
/// lands back as a `*Loaded` message; NONE of them block the iced UI/event-loop
/// thread, so the layer surface maps + `view()` paints its chrome and empty/
/// skeleton sections immediately, each filling when its `Loaded` arrives.
/// Mirrors the PR #50 applet idiom (`load_task` + `fetch_apps`). Cover art is NOT
/// dispatched here — it's gated behind `MusicLoaded` (point 4) so it never fires
/// on the first frame.
fn refresh_tasks() -> Task<Message> {
    Task::batch([
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_music)
                    .await
                    .ok()
                    .flatten()
            },
            Message::MusicLoaded,
        ),
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_voice)
                    .await
                    .ok()
                    .flatten()
            },
            Message::VoiceLoaded,
        ),
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_clips)
                    .await
                    .unwrap_or_default()
            },
            Message::ClipsLoaded,
        ),
        Task::perform(
            async { tokio::task::spawn_blocking(fetch_dnd).await.ok().flatten() },
            Message::DndLoaded,
        ),
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_lighthouses)
                    .await
                    .unwrap_or_default()
            },
            Message::LighthousesLoaded,
        ),
    ])
}

/// NOTIFY-RENDER-LAG-1 — dispatch the cover-art fetch+decode off-thread for
/// `cover_id`, landing back as `CoverArtLoaded`. Fired ONLY from the
/// `MusicLoaded` arm once the track's `coverArt` token is known (point 4), so the
/// ~1.2 s art round-trip never gates the first frame.
fn cover_art_task(cover_id: String) -> Task<Message> {
    Task::perform(
        async move {
            let handle = tokio::task::spawn_blocking({
                let id = cover_id.clone();
                move || fetch_cover_art(&id)
            })
            .await
            .ok()
            .flatten();
            (cover_id, handle)
        },
        |(id, handle)| Message::CoverArtLoaded(id, handle),
    )
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
    // NOTIFY-HUB-2 — only tick the new-item slide/blink while an entrance is
    // actually in flight (MOTION-PERF-1: an idle Hub does no animation work).
    if !s.anim.is_idle(std::time::Instant::now()) {
        subs.push(
            cosmic::iced::time::every(std::time::Duration::from_millis(ANIM_TICK_MS))
                .map(|_| Message::AnimTick),
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
            // NOTIFY-3 (L656) — apply the persisted read/cleared cursors to the
            // freshly-merged rows BEFORE the entrance animation samples them: a
            // cleared item is dropped (never animates in then vanishes) and an
            // already-read item renders acknowledged, so a relaunch honors the
            // operator's last clear-all / mark-all-read instead of resurrecting
            // the backlog.
            state.apply_read_state();
            // NOTIFY-HUB-2 — kick the slide-in + 2× severity blink for any rows
            // that just appeared (skipped on the first poll so opening the panel
            // doesn't strobe the whole backlog). Driven by the cheap local tail
            // merge above, so it stays in the synchronous part of the arm.
            state.note_new_items(std::time::Instant::now());
            // NOTIFY-RENDER-LAG-1 — the Music / Voice / Clipboard / DND /
            // Lighthouse snapshots are the slow part (each a blocking bus
            // round-trip, up to ~5.3 s of cumulative timeout budget). Dispatch
            // them off the UI thread as `Task::perform(spawn_blocking(...))`; each
            // lands back as its `*Loaded` message and fills its section then. The
            // layer surface therefore maps + paints immediately on the first
            // Refresh instead of stalling on these fetches. Cover art is NOT
            // dispatched here — it's gated behind `MusicLoaded` so it never fires
            // on the first frame.
            return refresh_tasks();
        }
        Message::MusicLoaded(music) => {
            state.music = music;
            // MUSIC-HUB-2 / point 4 — (re)fetch the cover art only AFTER music is
            // known and only when the track's coverArt token changed (so the
            // ~1.2 s art round-trip never gates the first frame, and re-fetches
            // only on a track change). A track with no art clears the thumbnail.
            let cover = state.music.as_ref().and_then(|m| m.cover_art.clone());
            if cover != state.now_art_id {
                state.now_art_id = cover.clone();
                match cover {
                    Some(id) => {
                        // Drop the stale art now so the placeholder shows until the
                        // new art lands; the off-thread decode fills it back in.
                        state.now_art = None;
                        return cover_art_task(id);
                    }
                    None => state.now_art = None,
                }
            }
        }
        Message::CoverArtLoaded(id, handle) => {
            // Apply only if this is still the current track's art (a fast track
            // change could land an older fetch after a newer one was dispatched).
            if state.now_art_id.as_deref() == Some(id.as_str()) {
                state.now_art = handle;
            }
        }
        Message::VoiceLoaded(voice) => state.voice = voice,
        Message::ClipsLoaded(clips) => state.clips = clips,
        // `None` = no Bus data dir → leave the live toggle as-is (the old inline
        // read only assigned `dnd_active` when `client_data_dir()` was `Some`).
        Message::DndLoaded(active) => {
            if let Some(active) = active {
                state.dnd_active = active;
            }
        }
        Message::LighthousesLoaded(beacons) => state.lighthouses = beacons,
        Message::AnimTick => {
            // Drop finished entrances; the subscription self-stops once idle.
            state.anim.gc(std::time::Instant::now());
        }
        Message::ToggleStack(key) => {
            if !state.expanded_stacks.remove(&key) {
                state.expanded_stacks.insert(key);
            }
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
            // NOTIFY-RENDER-LAG-1 — the transport command + its follow-up
            // get-state are blocking bus round-trips (~1.4 s worst case), so run
            // them off the UI thread and reflect the new state via `MusicLoaded`
            // (which also re-resolves the cover art). The button stays responsive.
            return Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        let _ = mde_workbench::dbus::action_request(
                            verb,
                            std::time::Duration::from_millis(700),
                        );
                        fetch_music()
                    })
                    .await
                    .ok()
                    .flatten()
                },
                Message::MusicLoaded,
            );
        }
        Message::ClipCopy(text) => {
            // CLIP-VIEW-1 — load the entry onto THIS node's clipboard. No re-list:
            // the capture worker re-syncs/debounces it, and the entry is already
            // at/near the top of the shared history.
            load_into_clipboard(&text);
        }
        Message::ClipTogglePin(id, pinned) => {
            // Currently pinned → unpin; else pin. Re-list reflects the mesh edit.
            let verb = if pinned { "unpin" } else { "pin" };
            state.clips = clip_mutate(verb, Some(&id));
        }
        Message::ClipDelete(id) => {
            state.clips = clip_mutate("delete", Some(&id));
        }
        Message::ClipClearAll => {
            // Mesh-wide clear of unpinned entries; pinned survive everywhere.
            state.clips = clip_mutate("clear", None);
        }
        Message::ToggleGroup(label) => {
            if !state.collapsed.remove(&label) {
                state.collapsed.insert(label);
            }
        }
        Message::MarkAllRead => {
            // NOTIFY-3 (L656) — advance the read cursor to the newest shown alert
            // and persist it, so the acknowledgement survives a restart (the bus
            // tail would otherwise re-surface this backlog as unread on relaunch).
            if let Some(newest) = state.items.iter().map(|i| i.id.clone()).max() {
                state.read_state.mark_read_through(&newest);
                state.persist_read_state();
            }
            for it in &mut state.items {
                it.read = true;
            }
        }
        Message::ClearAll => {
            // NOTIFY-3 (L656) — advance the cleared cursor past the newest shown
            // alert and persist it, so the dismissal survives a restart (without
            // it the bus tail + shared dir re-read the same backlog into the panel
            // on relaunch). Then drop the live rows.
            if let Some(newest) = state.items.iter().map(|i| i.id.clone()).max() {
                state.read_state.mark_cleared_through(&newest);
                state.persist_read_state();
            }
            state.items.clear();
        }
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
    let p = hub_palette();
    let now = now_ms();
    // NOTIFY-HUB-2 — single clock read for this frame's slide/blink sampling.
    let anim_now = std::time::Instant::now();

    // Header: title + close on the top line, the bulk actions on their own
    // line below so a long "· N unread" title never collides with the buttons
    // (the panel is only ~390px wide). Generous top/side padding so nothing is
    // jammed against the window edge.
    let unread = state.items.iter().filter(|i| !i.read).count();
    // NOTIFY-HUB-1 — a Carbon header matching the Application Menu's "▦ Applications"
    // header: an accent glyph + the title in heading size, with the unread count
    // as a muted suffix.
    let title_row = row![
        text("\u{25D4}\u{FE0E}") // ◔ — a notification/bell-ish BMP glyph (not emoji)
            .size(18)
            .color(p.accent.into_cosmic_color()),
        Space::new().width(Length::Fixed(10.0)),
        text("Notifications")
            .size(18)
            .color(p.text.into_cosmic_color()),
        Space::new().width(Length::Fixed(8.0)),
        text(format!("· {unread} unread"))
            .size(12)
            .color(p.text_muted.into_cosmic_color()),
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
            .width(Length::Fill)
            // Flat: a section-header toggle, not a chrome button (no blue box).
            .style(|_t, _s| cosmic::iced::widget::button::Style {
                background: None,
                ..Default::default()
            });
            body = body.push(head);
            if !collapsed {
                // NOTIFY-HUB-2 — fold same-source + same-title repeats into one
                // card carrying a count (expandable), then render each stack with
                // its slide/blink motion.
                for (i, stack) in collapse_stacks(&group).into_iter().enumerate() {
                    let expanded = state.expanded_stacks.contains(&stack.key);
                    body = body.push(stack_card(
                        &stack,
                        expanded,
                        i,
                        now,
                        anim_now,
                        &state.anim,
                        p,
                    ));
                    // Expanded stack: show the individual repeats beneath the head
                    // (the head is items[0], so the rest start at index 1).
                    if expanded && stack.is_stacked() {
                        for (j, item) in stack.items.iter().enumerate().skip(1) {
                            body = body.push(stack_child_row(
                                item,
                                i + j,
                                now,
                                anim_now,
                                &state.anim,
                                p,
                            ));
                        }
                    }
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
            launch_tile("\u{2317}\u{FE0E}", "Workbench", "mde-workbench", p),
            launch_tile("\u{25A4}", "Files", "mde-files", p),
            launch_tile("\u{2699}\u{FE0E}", "Settings", "cosmic-settings", p),
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
        // CLIP-VIEW-1 — Clipboard Viewer, locked above Music + SIP.
        clipboard_section(&state.clips, now, p),
        section_divider(p),
        now_playing_section(state.music.as_ref(), state.now_art.as_ref(), p),
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

    // NOTIFY-FX-1 — the Hub-open reveal: the whole body starts a few px low and
    // rises to rest (Carbon panel-mount), rendered as top padding that decays to
    // 0 — iced 0.13 has no transform widget, so we offset layout instead (the
    // same translate-as-padding approach the Application Menu uses on open). The
    // offset is 0 once settled (and always 0 under reduce-motion), so there's no
    // residual layout shift at rest.
    let open_slide = state.anim.open_params(anim_now).translate_y.max(0.0);
    let content: Element<'_, Message> =
        container(cosmic::iced::widget::column(sections).spacing(0))
            .padding(Padding {
                top: open_slide,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    // BRAND-11 (operator 2026-06-19) — a faint MCNF logo watermark pinned to the
    // lower visible area of the Hub. The asset is pre-faded (~16% alpha) so it
    // reads as a watermark without harming legibility; it carries no pointer
    // handlers, so clicks pass through to the launch bar / quick actions beneath.
    let watermark: Element<'_, Message> = container(
        cosmic::iced::widget::image(cosmic::iced::widget::image::Handle::from_bytes(
            include_bytes!("../../../../../assets/brand/watermark.png").to_vec(),
        ))
        .width(Length::Fixed(148.0))
        .height(Length::Fixed(148.0)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(cosmic::iced::alignment::Horizontal::Center)
    .align_y(cosmic::iced::alignment::Vertical::Bottom)
    .padding(Padding::from([0u16, 0u16, 6u16, 0u16]))
    .into();
    cosmic::iced::widget::Stack::with_children(vec![content, watermark])
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

/// NOTIFY-AC / MUSIC-HUB-2 — the "Now Playing" media section, styled to match the
/// Music app's mini playback bar: a square album-art tile + a title/artist stack
/// + the transport controls. Honest idle state when nothing is playing.
fn now_playing_section(
    music: Option<&MusicNow>,
    art: Option<&cosmic::iced::widget::image::Handle>,
    p: Palette,
) -> Element<'static, Message> {
    let body: Element<'static, Message> = match music {
        Some(m) if m.active => {
            let toggle_glyph = if m.playing { "\u{2016}" } else { "\u{25B6}" };
            // MUSIC-HUB-2 — 40×40 art tile (real cover when available, else a
            // muted ♪ placeholder on the raised layer), mirroring the mini-player.
            let art_tile: Element<'static, Message> = match art {
                Some(h) => cosmic::iced::widget::image(h.clone())
                    .width(Length::Fixed(40.0))
                    .height(Length::Fixed(40.0))
                    .into(),
                None => container(text("♪").size(16).color(p.text_muted.into_cosmic_color()))
                    .width(Length::Fixed(40.0))
                    .height(Length::Fixed(40.0))
                    .align_x(cosmic::iced::alignment::Horizontal::Center)
                    .align_y(cosmic::iced::alignment::Vertical::Center)
                    .style(move |_| container::Style {
                        background: Some(cosmic::iced::Background::Color(
                            p.raised.into_cosmic_color(),
                        )),
                        ..Default::default()
                    })
                    .into(),
            };
            let meta = column![
                text(m.title.clone())
                    .size(13)
                    .color(p.text.into_cosmic_color()),
                text(m.artist.clone())
                    .size(11)
                    .color(p.text_muted.into_cosmic_color()),
            ]
            .spacing(1)
            .width(Length::Fill);
            row![
                art_tile,
                Space::new().width(Length::Fixed(10.0)),
                meta,
                transport_button("\u{25C0}", Message::MusicPrev, p),
                transport_button(toggle_glyph, Message::MusicToggle, p),
                transport_button("\u{25B6}", Message::MusicNext, p),
            ]
            .spacing(4)
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

/// CLIP-VIEW-1 — the Clipboard Viewer section: a header (clipboard glyph +
/// "Clipboard" + a mesh-wide "Clear all" that wipes unpinned entries) over the
/// mesh-global history rows. Each row is click-to-copy onto THIS node, with a
/// per-entry pin (★ toggle, exempt from the 50-cap) + delete (✕). Capped to a
/// handful of visible rows so the section stays compact in the Hub column; the
/// full history lives in the shared file. Empty state is honest.
///
/// `now_ms` is the frame clock in milliseconds — the same epoch-ms clock the
/// notifications list passes to [`format_age`], so the clipboard ages read off
/// the same bucket ladder.
fn clipboard_section(clips: &[ClipRow], now_ms: i64, p: Palette) -> Element<'static, Message> {
    /// Rows shown inline in the Hub (the rest stay in the shared history).
    const VISIBLE_ROWS: usize = 6;

    let mut header = row![
        text("\u{2398}") // ⎘ next-page / clipboard-ish glyph (BMP, not emoji)
            .size(13)
            .color(p.accent.into_cosmic_color()),
        Space::new().width(Length::Fixed(8.0)),
        text("Clipboard")
            .size(13)
            .color(p.text.into_cosmic_color())
            .width(Length::Fill),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    // Clear-all only when there is at least one unpinned entry to wipe (pinned
    // survive a clear, so an all-pinned history has nothing to clear).
    if clips.iter().any(|c| !c.pinned) {
        header = header.push(action_button("Clear all", Message::ClipClearAll, p));
    }

    let mut col = column![header].spacing(6);
    if clips.is_empty() {
        col = col.push(
            text("Clipboard history is empty.")
                .size(12)
                .color(p.text_muted.into_cosmic_color()),
        );
    } else {
        // Pinned first (they're the operator's kept clips), then the rest in
        // the daemon's newest-first order; cap the inline view.
        let mut ordered: Vec<&ClipRow> = clips.iter().filter(|c| c.pinned).collect();
        ordered.extend(clips.iter().filter(|c| !c.pinned));
        for (i, c) in ordered.iter().take(VISIBLE_ROWS).enumerate() {
            col = col.push(clip_row(c, i, now_ms, p));
        }
        if ordered.len() > VISIBLE_ROWS {
            col = col.push(
                text(format!("+{} more in history", ordered.len() - VISIBLE_ROWS))
                    .size(11)
                    .color(p.text_muted.into_cosmic_color()),
            );
        }
    }

    container(col)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .into()
}

/// CLIP-VIEW-1 — one clipboard row: a click-to-copy body (single-line preview +
/// "from <node> · <age>" sub-label) trailed by the pin + delete controls. Zebra
/// shaded to match the notification rows' idiom.
fn clip_row(c: &ClipRow, idx: usize, now_ms: i64, p: Palette) -> Element<'static, Message> {
    let base = if idx % 2 == 1 {
        p.surface
    } else {
        p.background
    };
    let preview = notify_clipboard::preview(&c.text, 44);
    // Age off the SAME format_age ladder as the notifications list: parse the
    // RFC3339 stamp → epoch-ms → format_age. An unparseable stamp falls back to
    // the current instant (renders "0s") rather than a panic.
    let then_ms = notify_clipboard::rfc3339_to_epoch(&c.time).map_or(now_ms, |s| s * 1000);
    let meta = notify_clipboard::meta_label(&c.source, &format_age(then_ms, now_ms));

    // The text + meta stack — the whole thing is a flat click-to-copy button so
    // a click loads the verbatim clip onto this node (ClipCopy carries the full
    // text, not the truncated preview).
    let body = column![
        text(preview).size(12).color(p.text.into_cosmic_color()),
        text(meta).size(10).color(p.text_muted.into_cosmic_color()),
    ]
    .spacing(1)
    .width(Length::Fill);
    let copy = button(body)
        .on_press(Message::ClipCopy(c.text.clone()))
        .width(Length::Fill)
        .padding(0)
        .style(|_t, _s| cosmic::iced::widget::button::Style {
            background: None,
            ..Default::default()
        });

    // Pin toggle: filled ★ (accent) when pinned, hollow ☆ (muted) when not.
    let (pin_glyph, pin_color) = if c.pinned {
        ("\u{2605}", p.accent) // ★
    } else {
        ("\u{2606}", p.text_muted) // ☆
    };
    let pin_btn = button(
        text(pin_glyph)
            .size(13)
            .color(pin_color.into_cosmic_color()),
    )
    .padding(Padding::from([2u16, 6u16]))
    .on_press(Message::ClipTogglePin(c.id.clone(), c.pinned))
    .style(|_t, _s| cosmic::iced::widget::button::Style {
        background: None,
        ..Default::default()
    });
    let del_btn = button(
        text("\u{2715}")
            .size(12)
            .color(p.text_muted.into_cosmic_color()),
    ) // ✕
    .padding(Padding::from([2u16, 6u16]))
    .on_press(Message::ClipDelete(c.id.clone()))
    .style(|_t, _s| cosmic::iced::widget::button::Style {
        background: None,
        ..Default::default()
    });

    container(
        row![copy, pin_btn, del_btn]
            .spacing(2)
            .align_y(cosmic::iced::Alignment::Center),
    )
    .padding(Padding::from([5u16, 8u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(cosmic::iced::Background::Color(base.into_cosmic_color())),
        ..Default::default()
    })
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
            cosmic::iced::widget::scrollable::Scrollbar::new()
                .width(4)
                .scroller_width(4),
        ),
    );

    container(column![header, Space::new().height(Length::Fixed(8.0)), strip,].spacing(0))
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
            background: Some(cosmic::iced::Background::Color(
                p.surface.into_cosmic_color(),
            )),
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

/// NOTIFY-HUB / coloring (operator 2026-06-18) — the subtle Carbon button look
/// the Application Menu uses (`cosmic::theme::Button::Standard`): a muted layer
/// fill (raised → overlay on hover) with a subtle border, NOT the default bright
/// blue "suggested" style. Applied to every Hub button so the Hub matches the
/// Application Menu's coloring pattern instead of being all-blue.
fn carbon_btn(
    p: Palette,
) -> impl Fn(&Theme, cosmic::iced::widget::button::Status) -> cosmic::iced::widget::button::Style {
    use cosmic::iced::widget::button::Status;
    move |_t, status| {
        let bg = match status {
            Status::Hovered | Status::Pressed => p.overlay,
            _ => p.raised,
        };
        cosmic::iced::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(bg.into_cosmic_color())),
            text_color: p.text.into_cosmic_color(),
            border: cosmic::iced::Border {
                color: p.border.into_cosmic_color(),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        }
    }
}

fn transport_button(glyph: &str, msg: Message, p: Palette) -> Element<'_, Message> {
    button(
        text(glyph.to_string())
            .size(14)
            .color(p.text.into_cosmic_color()),
    )
    .padding(Padding::from([4u16, 8u16]))
    .on_press(msg)
    .style(carbon_btn(p))
    .into()
}

/// A bottom quick-launch tile: label + the binary it spawns (`OpenApp`).
/// MUSIC-HUB-3 — a quick-launch tile matching the Application Menu's quick-link
/// tiles: a glyph over a centred label, equal-width. (The applet uses the same
/// ⌗ / ▤ / ⚙ glyphs for Workbench / Files / Settings.)
fn launch_tile<'a>(
    glyph: &'a str,
    label: &'a str,
    cmd: &'static str,
    p: Palette,
) -> Element<'a, Message> {
    button(
        cosmic::iced::widget::column![
            text(glyph.to_string())
                .size(18)
                .color(p.text.into_cosmic_color()),
            text(label).size(12).color(p.text.into_cosmic_color()),
        ]
        .spacing(6)
        .align_x(cosmic::iced::Alignment::Center)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .padding(Padding::from([8u16, 6u16]))
    .on_press(Message::OpenApp(cmd))
    .style(carbon_btn(p))
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
    .style(carbon_btn(p))
    .into()
}

/// The title/body column for one alert (severity glyph · title · age/host, plus
/// an optional body line). `count_badge` adds a "×N" pill for a collapsed stack
/// head. Shared by [`stack_card`] + [`stack_child_row`].
fn alert_row_body(
    item: &AlertItem,
    now_ms: i64,
    count_badge: Option<usize>,
    p: Palette,
) -> Element<'static, Message> {
    let sev_color = severity_token(item.severity, &p).into_cosmic_color();
    let title_color = if item.read { p.text_muted } else { p.text }.into_cosmic_color();
    let host = item.host.clone().unwrap_or_default();
    let meta = if host.is_empty() {
        format_age(item.ts_unix_ms, now_ms)
    } else {
        format!("{} · {host}", format_age(item.ts_unix_ms, now_ms))
    };
    let mut head = row![
        text(severity_glyph(item.severity))
            .size(13)
            .color(sev_color),
        Space::new().width(Length::Fixed(8.0)),
        text(item.title.clone()).size(13).color(title_color),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    // NOTIFY-HUB-2 — a "×N" repeat badge in the severity colour when this card
    // stands in for a collapsed same-source run.
    if let Some(n) = count_badge {
        if n > 1 {
            head = head.push(Space::new().width(Length::Fixed(6.0)));
            head = head.push(text(format!("\u{00d7}{n}")).size(11).color(sev_color));
        }
    }
    head = head.push(Space::new().width(Length::Fill));
    head = head.push(text(meta).size(11).color(p.text_muted.into_cosmic_color()));
    let mut col = column![head].spacing(2);
    if !item.body.is_empty() {
        let body: String = item.body.chars().take(200).collect();
        col = col.push(text(body).size(11).color(p.text_muted.into_cosmic_color()));
    }
    col.into()
}

/// NOTIFY-HUB-2 — composite the severity-blink wash over a base zebra shade:
/// alpha-over blend so the blink reads as a tint of the row, not an opaque flood.
fn blink_shade(base: mde_theme::Rgba, blink: mde_theme::Rgba) -> cosmic::iced::Color {
    let a = blink.a.clamp(0.0, 1.0);
    let mix = |b: u8, t: u8| -> f32 {
        let b = f32::from(b) / 255.0;
        let t = f32::from(t) / 255.0;
        b * (1.0 - a) + t * a
    };
    cosmic::iced::Color {
        r: mix(base.r, blink.r),
        g: mix(base.g, blink.g),
        b: mix(base.b, blink.b),
        a: 1.0,
    }
}

/// NOTIFY-HUB-2 — wrap an alert's body in its zebra-shaded card and apply the
/// per-card motion: a slide offset (rendered as left/top padding so iced 0.13
/// needs no transform widget) plus the severity-blink background wash. A card at
/// rest renders exactly as the pre-animation zebra row.
fn motioned_card(
    inner: Element<'static, Message>,
    idx: usize,
    severity: Severity,
    motion: motion::CardMotion,
    p: Palette,
) -> Element<'static, Message> {
    // Zebra base layer (the Application Menu row idiom).
    let base = if idx % 2 == 1 {
        p.surface
    } else {
        p.background
    };
    // A settled card draws as the plain zebra row (no blink, no offset) — the
    // common case once nothing is entering.
    let (bg, left, top) = if motion.is_rest() {
        (base.into_cosmic_color(), 0.0_f32, 0.0_f32)
    } else {
        // Severity-coloured blink wash over the base.
        let blink = motion::blink_tint(severity, &p, motion.blink_alpha);
        // Slide offsets → padding. left = still-to-the-right slide-in; top =
        // still-below slide-down.
        (
            blink_shade(base, blink),
            motion.translate_x.max(0.0),
            motion.translate_y.max(0.0),
        )
    };
    container(inner)
        .padding(Padding {
            top: top + 6.0,
            right: 8.0,
            bottom: 6.0,
            left: left + 8.0,
        })
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(cosmic::iced::Background::Color(bg)),
            ..Default::default()
        })
        .into()
}

/// NOTIFY-HUB-2 — a stack's head card: the representative alert with a count
/// badge + the slide/blink entrance motion. When the stack folds repeats, the
/// whole card is a toggle that expands/collapses the run.
fn stack_card(
    stack: &Stack,
    expanded: bool,
    idx: usize,
    now_ms: i64,
    anim_now: std::time::Instant,
    anim: &HubAnim,
    p: Palette,
) -> Element<'static, Message> {
    let m = anim.card_motion(&stack.head.id, anim_now);
    let badge = stack.is_stacked().then_some(stack.count());
    let mut body = alert_row_body(&stack.head, now_ms, badge, p);
    // Expandable run → a flat full-width toggle wrapping the card body.
    if stack.is_stacked() {
        let caret = if expanded { "\u{25be}" } else { "\u{25b8}" }; // ▾ / ▸
        let toggled = row![
            text(caret).size(11).color(p.text_muted.into_cosmic_color()),
            Space::new().width(Length::Fixed(6.0)),
            body,
        ]
        .align_y(cosmic::iced::Alignment::Center);
        body = button(toggled)
            .on_press(Message::ToggleStack(stack.key.clone()))
            .width(Length::Fill)
            .padding(0)
            .style(|_t, _s| cosmic::iced::widget::button::Style {
                background: None,
                ..Default::default()
            })
            .into();
    }
    motioned_card(body, idx, stack.head.severity, m, p)
}

/// NOTIFY-HUB-2 — one revealed repeat under an expanded stack head. Slightly
/// indented; carries the same slide-down motion as the head's siblings (it is
/// never itself the *new* item — the head is) so the reveal moves coherently.
fn stack_child_row(
    item: &AlertItem,
    idx: usize,
    now_ms: i64,
    anim_now: std::time::Instant,
    anim: &HubAnim,
    p: Palette,
) -> Element<'static, Message> {
    let m = anim.card_motion(&item.id, anim_now);
    let indented = row![
        Space::new().width(Length::Fixed(16.0)),
        alert_row_body(item, now_ms, None, p),
    ];
    motioned_card(indented.into(), idx, item.severity, m, p)
}

fn action_button<'a>(label: &'a str, msg: Message, p: Palette) -> Element<'a, Message> {
    button(text(label).size(12).color(p.text.into_cosmic_color()))
        .padding(Padding::from([4u16, 10u16]))
        .on_press(msg)
        .style(carbon_btn(p))
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

    /// NOTIFY-RENDER-LAG-1 — a non-default `MusicNow` sentinel the snapshot
    /// fields are pre-seeded with so a test can prove `Refresh` does NOT overwrite
    /// them inline (the old blocking-fetch behavior).
    fn music_sentinel() -> MusicNow {
        MusicNow {
            active: true,
            playing: true,
            title: "SENTINEL".into(),
            artist: "SENTINEL".into(),
            audio_available: true,
            needs_airsonic: false,
            cover_art: None,
        }
    }

    /// A minimal healthy beacon for the snapshot-section tests (`Beacon` has no
    /// `Default`).
    fn beacon_sentinel() -> Beacon {
        Beacon {
            hostname: "lh-sentinel".into(),
            overlay_ip: None,
            is_master: false,
            status: lighthouse::BeaconStatus::Healthy,
        }
    }

    /// NOTIFY-RENDER-LAG-1 — the core render-lag regression guard: the `Refresh`
    /// arm must ONLY dispatch the off-thread fetch Tasks, never run the blocking
    /// bus fetches inline. We prove this behaviorally: pre-seed every snapshot
    /// section with a sentinel, run `Refresh`, and assert the sentinels survive.
    /// With the old inline `state.music = fetch_music()` (etc.) chain, `Refresh`
    /// would have clobbered each section with the fetch result (empty/default in
    /// the test env, no Bus daemon); now those fields only change via the matching
    /// `*Loaded` arm, so a `Refresh` leaves them untouched and the slow round-trips
    /// are entirely off the UI thread.
    #[test]
    fn refresh_does_not_fetch_sections_inline() {
        let mut state = Center::new();
        state.music = Some(music_sentinel());
        state.voice = Some(VoiceStatus {
            registered: true,
            listening: true,
            detail: "SENTINEL".into(),
            fresh: true,
        });
        state.clips = vec![ClipRow {
            id: "sentinel".into(),
            text: "SENTINEL".into(),
            source: "node".into(),
            time: String::new(),
            pinned: false,
        }];
        state.lighthouses = vec![beacon_sentinel()];
        state.dnd_active = true;
        state.now_art_id = Some("sentinel-art".into());

        let _task = update(&mut state, Message::Refresh);

        // None of the slow sections were touched inline — they still hold the
        // pre-seeded sentinels (they would only change via a `*Loaded` message).
        assert_eq!(
            state.music.as_ref().map(|m| m.title.as_str()),
            Some("SENTINEL")
        );
        assert_eq!(
            state.voice.as_ref().map(|v| v.detail.as_str()),
            Some("SENTINEL")
        );
        assert_eq!(
            state.clips.len(),
            1,
            "clips not re-listed inline by Refresh"
        );
        assert_eq!(state.clips[0].text, "SENTINEL");
        assert_eq!(
            state.lighthouses.len(),
            1,
            "lighthouses not refetched inline"
        );
        assert!(state.dnd_active, "DND not re-read inline by Refresh");
        // Cover art is gated behind MusicLoaded — Refresh never touches it.
        assert_eq!(state.now_art_id.as_deref(), Some("sentinel-art"));
    }

    /// NOTIFY-RENDER-LAG-1 — a `*Loaded` arm stores its result into state without
    /// blocking. `MusicLoaded` is the representative case (it also drives the
    /// gated cover-art dispatch); the others are the same trivial store.
    #[test]
    fn loaded_arm_stores_state() {
        let mut state = Center::new();
        assert!(state.music.is_none());
        let _ = update(&mut state, Message::MusicLoaded(Some(music_sentinel())));
        assert_eq!(
            state.music.as_ref().map(|m| m.title.as_str()),
            Some("SENTINEL")
        );

        let _ = update(&mut state, Message::DndLoaded(Some(true)));
        assert!(state.dnd_active);
        // A `None` DND load (no Bus dir) leaves the toggle UNCHANGED, not cleared.
        let _ = update(&mut state, Message::DndLoaded(None));
        assert!(
            state.dnd_active,
            "DndLoaded(None) must not clear the toggle"
        );

        let _ = update(
            &mut state,
            Message::LighthousesLoaded(vec![beacon_sentinel()]),
        );
        assert_eq!(state.lighthouses.len(), 1);
    }

    /// NOTIFY-RENDER-LAG-1 / point 4 — cover art is fetched only AFTER music is
    /// known and only when the track's coverArt token changes. A `MusicLoaded`
    /// with no `cover_art` must NOT set `now_art_id` to a fetched token, and a
    /// `CoverArtLoaded` only applies when it still matches the current track.
    #[test]
    fn cover_art_is_gated_on_music_and_track() {
        let mut state = Center::new();
        // Music with no art: now_art_id stays None (no first-frame art fetch).
        let _ = update(&mut state, Message::MusicLoaded(Some(music_sentinel())));
        assert_eq!(state.now_art_id, None);
        assert!(state.now_art.is_none());

        // Music with an art token: now_art_id advances to it (the fetch Task is
        // dispatched off-thread, not awaited here).
        let mut m = music_sentinel();
        m.cover_art = Some("art-A".into());
        let _ = update(&mut state, Message::MusicLoaded(Some(m)));
        assert_eq!(state.now_art_id.as_deref(), Some("art-A"));

        // A late CoverArtLoaded for a STALE token is ignored (track moved on).
        let _ = update(
            &mut state,
            Message::CoverArtLoaded("art-OLD".into(), Some(stub_handle())),
        );
        assert!(
            state.now_art.is_none(),
            "stale cover art must not overwrite the current track's art slot"
        );

        // A CoverArtLoaded for the CURRENT token applies.
        let _ = update(
            &mut state,
            Message::CoverArtLoaded("art-A".into(), Some(stub_handle())),
        );
        assert!(state.now_art.is_some());
    }

    /// A throwaway image handle for the cover-art gating test. The bytes are
    /// never decoded — the test only checks the handle lands in the art slot — so
    /// any non-empty buffer suffices (mirrors `fetch_cover_art`'s
    /// `Handle::from_bytes`).
    fn stub_handle() -> cosmic::iced::widget::image::Handle {
        cosmic::iced::widget::image::Handle::from_bytes(vec![0u8, 1, 2, 3])
    }

    /// NOTIFY-RENDER-LAG-1 — the first frame paints immediately: `view()` on a
    /// fresh `Center` (no fetch has completed — every section is empty/skeleton)
    /// builds a renderable Element without panicking. This is the user-visible
    /// payoff — the layer surface's chrome + empty sections render before any of
    /// the off-thread fetches land.
    #[test]
    fn first_view_renders_without_any_fetch() {
        let state = Center::new();
        // No fetches have run, so the snapshot sections are all empty.
        assert!(state.music.is_none());
        assert!(state.voice.is_none());
        assert!(state.clips.is_empty());
        assert!(state.lighthouses.is_empty());
        // Building the view must succeed (chrome + skeleton sections) — this is
        // what the operator sees on the first frame, before any Loaded lands.
        let _element = view(&state, window::Id::unique());
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

    // ── NOTIFY-3 (L656): persisted read/cleared wiring ──────────────────

    /// Mark-all-read advances the read cursor to the newest shown alert id and
    /// flags every current row read — the cursor is what survives a restart.
    #[test]
    fn mark_all_read_advances_cursor_and_flags_rows() {
        let mut state = Center::new();
        state.read_state = mde_notify::ReadState::default();
        state.items = vec![
            item("01A", Source::System, Severity::Info, 1),
            item("01C", Source::Security, Severity::Critical, 3),
            item("01B", Source::System, Severity::Info, 2),
        ];
        let _ = update(&mut state, Message::MarkAllRead);
        // The cursor is the lexicographically-greatest (newest) id.
        assert_eq!(state.read_state.read_through, "01C");
        assert!(state.items.iter().all(|i| i.read), "all rows acknowledged");
    }

    /// Clear-all advances the cleared cursor and drops the rows; a subsequent poll
    /// that re-reads the same backlog off the bus tail must stay hidden, while a
    /// newer alert (greater ULID) still surfaces — the restart-survival contract.
    #[test]
    fn clear_all_advances_cursor_and_hides_replayed_backlog() {
        let mut state = Center::new();
        state.read_state = mde_notify::ReadState::default();
        state.items = vec![
            item("01A", Source::System, Severity::Info, 1),
            item("01C", Source::Security, Severity::Info, 3),
        ];
        let _ = update(&mut state, Message::ClearAll);
        assert!(state.items.is_empty(), "clear-all empties the live list");
        assert_eq!(state.read_state.cleared_through, "01C");
        // Simulate a relaunch's re-read: the bus tail re-surfaces the cleared ids
        // plus a brand-new alert. apply_read_state must hide the cleared backlog
        // and keep the newer one.
        state.items = vec![
            item("01A", Source::System, Severity::Info, 1), // cleared
            item("01C", Source::Security, Severity::Info, 3), // cleared (inclusive)
            item("01D", Source::Firewall, Severity::Warning, 4), // newer → survives
        ];
        state.apply_read_state();
        let ids: Vec<&str> = state.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["01D"], "only the post-clear alert remains");
    }

    /// On a relaunch the read cursor flags the re-read backlog acknowledged (so it
    /// doesn't all come back unread), while newer alerts stay unread.
    #[test]
    fn read_cursor_marks_replayed_backlog_acknowledged() {
        let mut state = Center::new();
        state.read_state = mde_notify::ReadState::default();
        state.read_state.mark_read_through("01C");
        state.items = vec![
            item("01B", Source::System, Severity::Info, 2), // <= cursor → read
            item("01D", Source::System, Severity::Info, 4), // >  cursor → unread
        ];
        state.apply_read_state();
        assert!(state.items[0].read, "backlog at/below the cursor is read");
        assert!(!state.items[1].read, "a newer alert stays unread");
    }
}
