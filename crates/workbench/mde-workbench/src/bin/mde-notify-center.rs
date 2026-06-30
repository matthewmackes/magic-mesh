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
use mde_notify::{severity_token, AlertItem, AlertTail, Severity};
use mde_theme::{mde_icon, Icon, IconSize, Palette};

use motion::HubAnim;
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
/// NOTIFY-REDESIGN-B — the dim floor of the in-call Voice icon's blink: its glyph
/// alpha breathes between this and full opacity (an opacity channel — there is no
/// opacity widget in the iced fork — so the floor is never fully dark and the
/// glyph stays legible through the blink). A wash, like the per-item blink peak.
const VOICE_BLINK_MIN_ALPHA: f32 = 0.35;
/// NOTIFY-REDESIGN-B — the Voice/Music icon tile geometry (a compact peer of the
/// 54/78 px lighthouse `beacon_card`, sized to sit two-up beside the beacon strip).
/// Component dimensions, not density-scaled — single-sourced here so both cards
/// share one geometry rather than scattering the literals.
const VM_ICON_SQUARE_PX: f32 = 48.0;
/// Base glyph size inside the tile (the pulse scales up from this).
const VM_ICON_GLYPH_PX: f32 = 22.0;
/// Full card width (tile + caption column).
const VM_ICON_CARD_PX: f32 = 56.0;

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
    /// True when the snapshot's `ts` is within the staleness window (live agent).
    fresh: bool,
}

/// NOTIFY-REDESIGN-B — the rendered state of the footer **Music** icon, derived
/// from the live `MusicNow` snapshot. Drives colour + motion + show/hide: a
/// playing track pulses in the accent, a paused track is static accent, a
/// present-but-idle daemon greys out, and an absent/offline daemon HIDES the icon
/// (the lighthouse-footer hide-when-empty convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MusicIconState {
    /// A track is playing — accent, gentle pulse.
    Playing,
    /// A track is loaded but paused — accent, static.
    Paused,
    /// Daemon present but nothing playing (idle / needs setup) — muted, static.
    Idle,
    /// No music daemon reachable — the icon is hidden.
    Hidden,
}

impl MusicIconState {
    /// The §4 palette token the icon's glyph reads in (a present icon is always
    /// drawn in a token; `Hidden` never renders so its colour is unused).
    fn token(self, p: &Palette) -> mde_theme::Rgba {
        match self {
            MusicIconState::Playing | MusicIconState::Paused => p.accent,
            MusicIconState::Idle | MusicIconState::Hidden => p.text_muted,
        }
    }

    /// `true` only while a track is playing — the single state that animates
    /// (gentle pulse). Paused/idle are static.
    fn pulses(self) -> bool {
        matches!(self, MusicIconState::Playing)
    }
}

/// NOTIFY-REDESIGN-B — map the live Now-Playing snapshot to the Music icon's
/// render state. `None` (no snapshot / daemon down) hides the icon; a present
/// snapshot greys when idle, shows accent when a track is loaded, and pulses only
/// while actually playing. Pure + testable.
fn music_icon_state(music: Option<&MusicNow>) -> MusicIconState {
    match music {
        Some(m) if m.active && m.playing => MusicIconState::Playing,
        Some(m) if m.active => MusicIconState::Paused,
        Some(_) => MusicIconState::Idle,
        None => MusicIconState::Hidden,
    }
}

/// NOTIFY-REDESIGN-B — the rendered state of the footer **Voice** icon, derived
/// from the live `VoiceStatus` snapshot. An on-call agent blinks in the accent, a
/// registered/ready agent shows the ok-tone, a present-but-unregistered agent
/// greys, and a stale/absent agent HIDES the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceIconState {
    /// On a call (the agent is listening) — accent, blink.
    InCall,
    /// Registered / ready, not on a call — ok-tone, static.
    Ready,
    /// Agent present but not registered — muted, static.
    Idle,
    /// No live voice agent — the icon is hidden.
    Hidden,
}

impl VoiceIconState {
    /// The §4 palette token the icon's glyph reads in.
    fn token(self, p: &Palette) -> mde_theme::Rgba {
        match self {
            VoiceIconState::InCall => p.accent,
            VoiceIconState::Ready => p.success,
            VoiceIconState::Idle | VoiceIconState::Hidden => p.text_muted,
        }
    }

    /// `true` only while on a call — the single state that animates (blink).
    fn blinks(self) -> bool {
        matches!(self, VoiceIconState::InCall)
    }
}

/// NOTIFY-REDESIGN-B — map the live voice snapshot to the Voice icon's render
/// state. A stale/absent agent (`None` / `!fresh`) hides the icon; a live agent
/// blinks while on a call, shows the ok-tone when registered, and greys when
/// present-but-unregistered. Pure + testable.
fn voice_icon_state(voice: Option<&VoiceStatus>) -> VoiceIconState {
    match voice {
        Some(v) if v.fresh && v.listening => VoiceIconState::InCall,
        Some(v) if v.fresh && v.registered => VoiceIconState::Ready,
        Some(v) if v.fresh => VoiceIconState::Idle,
        _ => VoiceIconState::Hidden,
    }
}

/// NOTIFY-REDESIGN-B — the 0..1 loop phase for the ambient Voice/Music icon
/// motion at lighthouse beam step `beam_step`, over a loop of `period`. It rides
/// the SAME beam-tick clock the lighthouse beacons sweep on ([`BEAM_TICK_MS`]), so
/// the footer's two labeled sections animate off one shared motion clock (the
/// beacon idiom, reused not reforked). `period` is always an `mde_theme::motion`
/// token, so there is no bespoke beat (§4). Pure + testable.
fn vm_phase(beam_step: u16, period: std::time::Duration) -> f32 {
    let period_ms = (period.as_secs_f32() * 1000.0).max(f32::EPSILON);
    let elapsed_ms = f32::from(beam_step) * BEAM_TICK_MS as f32;
    ((elapsed_ms % period_ms) / period_ms).clamp(0.0, 1.0)
}

/// NOTIFY-REDESIGN-B — the in-call Voice blink's glyph alpha at loop `phase`: an
/// eased ping-pong between [`VOICE_BLINK_MIN_ALPHA`] and full opacity. Reuses the
/// shared `pulse_scale` ping-pong (`= lerp(1, max, eased-triangle)`); scaling its
/// `[1, 1/floor]` output by `floor` yields `lerp(floor, 1, eased-triangle)` — the
/// alpha breathe with no second tween implementation (§6). Pure + testable.
fn blink_alpha(phase: f32) -> f32 {
    VOICE_BLINK_MIN_ALPHA * mde_theme::animation::pulse_scale(phase, 1.0 / VOICE_BLINK_MIN_ALPHA)
}

/// NOTIFY-REDESIGN-A — the two top tabs. The Notifications tab (default) holds
/// the flat alert list; the Clipboard tab holds the mesh clipboard viewer. The
/// status footer renders under both, regardless of which tab is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Notifications,
    Clipboard,
}

struct Center {
    items: Vec<AlertItem>,
    tail: AlertTail,
    /// NOTIFY-REDESIGN-A — the active top tab. Switching it changes only the
    /// scrollable content; the status footer (severity strip + lighthouses)
    /// stays rendered on both.
    tab: Tab,
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
    /// CLIP-VIEW-1 — the mesh-global clipboard history (newest first), refreshed
    /// each poll from `action/clipboard/list`. Empty until the first list reply /
    /// when the daemon is down. Each row renders as text + source-node + age, is
    /// click-to-copy onto THIS node, and carries per-entry pin + delete.
    clips: Vec<ClipRow>,
    /// NOTIFY-FX-1 — the instant the Hub surface opened, so its open-in animation
    /// (the launcher's `panel_mount` slide-up, folded in here for one shared
    /// effects vocabulary) eases from this start. Set in [`Center::new`].
    opened_at: std::time::Instant,
    /// NOTIFY-FX-1 — `a11y.reduce_motion` resolved from the user's preferences, so
    /// the shared open-in helper collapses the slide to a pure crossfade (the same
    /// reduce-motion contract `HubAnim` already honors for the per-item motion).
    reduce_motion: bool,
    /// NOTIFY-REDESIGN-B — whether the Music icon's now-playing popover is open.
    /// The footer Music ICON replaced the old full-width Now-Playing bar; the
    /// track + transport now live in this on-demand popover (toggled by clicking
    /// the icon), so the resting footer stays a compact two-icon strip.
    music_popover: bool,
}

impl Center {
    fn new() -> Self {
        let reduce_motion = mde_theme::Preferences::load().a11y.reduce_motion;
        Self {
            items: Vec::new(),
            tail: AlertTail::default(),
            tab: Tab::default(),
            music: None,
            voice: None,
            dnd_active: false,
            shared_rx: None,
            lighthouses: Vec::new(),
            beam_step: 0,
            now_art: None,
            now_art_id: None,
            anim: HubAnim::new(reduce_motion),
            seen_ids: HashSet::new(),
            clips: Vec::new(),
            // NOTIFY-FX-1 — the open-in clock starts the moment the Center is
            // constructed (the layer surface is mapped right after), so the Hub
            // slides up on open exactly like the launcher dropdown.
            opened_at: std::time::Instant::now(),
            reduce_motion,
            music_popover: false,
        }
    }

    /// NOTIFY-REDESIGN-B — does the footer Voice/Music icon need the ambient
    /// motion tick? True only when a track is actually playing (the Music pulse)
    /// or a call is live (the Voice blink), and motion isn't reduced — so an idle
    /// footer (or a reduce-motion preference) arms no extra tick, exactly like the
    /// lighthouse beam tick self-disarms on an empty footer (MOTION-PERF-1).
    fn vm_motion_active(&self) -> bool {
        !self.reduce_motion
            && (music_icon_state(self.music.as_ref()).pulses()
                || voice_icon_state(self.voice.as_ref()).blinks())
    }

    /// NOTIFY-FX-1 — the Hub-open transition at `now`, reusing the **same** shared
    /// vocabulary the launcher's open-in uses (`mde_theme::animation::slide_in` =
    /// `Motion::panel_mount` slide-up, reduce-motion ⇒ crossfade). This is GLUE
    /// over the shared helper — the launcher's `menu_in()` resolves the identical
    /// `Transition::SlideUp(PANEL_MOUNT_TRANSLATE_Y_PX)`, so Hub + Application Menu
    /// open with one motion idiom (NOTIFY-FX-1 acceptance). A settled/disabled
    /// motion returns rest (no offset), so once the open beat is over the Hub
    /// renders exactly as before.
    fn open_in(&self, now: std::time::Instant) -> mde_theme::animation::RenderParams {
        mde_theme::animation::slide_in(
            self.opened_at,
            now,
            mde_theme::PANEL_MOUNT_TRANSLATE_Y_PX,
            self.reduce_motion,
        )
    }

    /// NOTIFY-FX-1 — is the open-in slide still in flight at `now`? Gates the
    /// animation tick so the Hub keeps ticking through the open beat (then stops),
    /// same as it does for an in-flight per-item entrance (MOTION-PERF-1: no idle
    /// wakeups once both are settled).
    fn open_in_flight(&self, now: std::time::Instant) -> bool {
        // `panel_mount` is the longest beat in play; once it's elapsed the slide
        // has settled. Under reduce-motion the helper caps the duration, but the
        // ≤80 ms window is still bounded by the full beat, so this gate is safe.
        now.duration_since(self.opened_at) < mde_theme::Motion::panel_mount().duration
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
    /// NOTIFY-REDESIGN-A — switch the active top tab (Notifications / Clipboard).
    SwitchTab(Tab),
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
    /// NOTIFY-REDESIGN-B — show/hide the Music icon's now-playing popover. NOT a
    /// transport verb — it only reveals the popover that hosts the reused
    /// `MusicPrev`/`MusicToggle`/`MusicNext` controls (the compact replacement for
    /// the removed full-width Now-Playing bar).
    MusicTogglePopover,
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
    // LIGHTHOUSE-3 / NOTIFY-REDESIGN-B — the beam tick drives BOTH the lighthouse
    // beacon sweep AND the Voice/Music icon pulse/blink (one shared motion clock).
    // Arm it when at least one lighthouse is shown OR a track is playing / a call
    // is live; an idle footer (no beacons, nothing playing, no call) costs no CPU
    // (Q12 "inactive when hidden"; reduce-motion never arms it either).
    let mut subs = vec![poll, esc];
    if !s.lighthouses.is_empty() || s.vm_motion_active() {
        subs.push(
            cosmic::iced::time::every(std::time::Duration::from_millis(BEAM_TICK_MS))
                .map(|_| Message::BeamTick),
        );
    }
    // NOTIFY-HUB-2 / NOTIFY-FX-1 — tick the per-item slide/blink while an entrance
    // is in flight, AND tick through the Hub-open slide-up beat so the launcher's
    // folded-in open-in animates to rest. One shared `AnimTick` drives both; at
    // rest neither is armed (MOTION-PERF-1: an idle Hub does no animation work).
    let now = std::time::Instant::now();
    if !s.anim.is_idle(now) || s.open_in_flight(now) {
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
        Message::SwitchTab(tab) => state.tab = tab,
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
        Message::MusicTogglePopover => {
            // NOTIFY-REDESIGN-B — flip the now-playing popover open/closed. Pure
            // UI state; the next `view()` shows/hides the card (only when the
            // music daemon is actually reachable — see the render gate).
            state.music_popover = !state.music_popover;
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

/// NOTIFY-REDESIGN-A — the flat, chronological (newest-first) alert list. The
/// redesign drops the per-source grouping AND the same-source stacking, so the
/// Hub shows ONE time-ordered stream; source identity moves to the (later)
/// click-opened detail viewer. Sorting here (rather than relying on the insert
/// order, which differs between the local tail and the shared-merge paths)
/// guarantees a stable newest-first order whatever the merge route. Pure +
/// testable.
#[must_use]
pub fn flat_newest_first(items: &[AlertItem]) -> Vec<AlertItem> {
    let mut v = items.to_vec();
    v.sort_by(|a, b| b.ts_unix_ms.cmp(&a.ts_unix_ms));
    v
}

/// NOTIFY-STATUS-STRIP — render a Carbon (Material Symbols) icon as a tinted SVG
/// **in this bin**. The notify daemons sit on the far side of the World-1/World-2
/// `Theme` boundary, so they can't call the `panel_chrome` icon renderers; this
/// is the in-bin mirror of that idiom — `mde_icon(..).svg_bytes()` →
/// `cosmic::iced::widget::svg` with a token-coloured `svg::Style`. The tint is
/// always a `Palette`/severity token the caller passes (§4 — no raw colour here).
fn icon_svg(icon: Icon, size: IconSize, color: cosmic::iced::Color) -> Element<'static, Message> {
    use cosmic::iced::widget::svg as widget_svg;
    let resolved = mde_icon(icon, size);
    let px = resolved.size_px();
    match resolved.svg_bytes() {
        Some(bytes) => widget_svg(widget_svg::Handle::from_memory(bytes))
            .width(Length::Fixed(px))
            .height(Length::Fixed(px))
            .style(move |_t, _s| widget_svg::Style { color: Some(color) })
            .into(),
        // Structural fallback (svg_bytes is always Some today) — never a raw glyph.
        None => text(resolved.fallback_glyph).size(px).color(color).into(),
    }
}

/// NOTIFY-STATUS-STRIP — the Carbon status icon for a severity. The four glyphs
/// are **shape-distinct** (error octagon / warning triangle / info & check
/// circles), so the severity reads without relying on colour alone (a11y axis 6
/// — never colour-only). Pure + testable.
fn severity_icon(s: Severity) -> Icon {
    match s {
        Severity::Critical => Icon::StatusError,
        Severity::Warning => Icon::StatusWarning,
        Severity::Info => Icon::StatusInfo,
        Severity::Success => Icon::StatusOk,
    }
}

/// NOTIFY-REDESIGN-A — the ambient severity strip in the status footer: a compact
/// per-severity tally (shape-distinct Carbon status icon + a count) over the
/// alerts currently held, most-severe first. It is rendered OUTSIDE the tab-
/// switched scroll area, so the mesh's alert posture stays visible on BOTH tabs
/// (the operator sees it while on the Clipboard tab). The shape-distinct icons
/// carry severity without relying on colour (a11y, the NOTIFY-STATUS-STRIP rule);
/// an empty hub shows an honest "All clear" rather than a blank strip.
fn severity_strip(items: &[AlertItem], p: Palette) -> Element<'static, Message> {
    let mut strip = row![].spacing(14).align_y(cosmic::iced::Alignment::Center);
    let mut any = false;
    for s in [
        Severity::Critical,
        Severity::Warning,
        Severity::Info,
        Severity::Success,
    ] {
        let n = items.iter().filter(|i| i.severity == s).count();
        if n == 0 {
            continue;
        }
        any = true;
        let color = severity_token(s, &p).into_cosmic_color();
        strip = strip.push(
            row![
                icon_svg(severity_icon(s), IconSize::Inline, color),
                Space::new().width(Length::Fixed(4.0)),
                text(n.to_string()).size(12).color(color),
            ]
            .align_y(cosmic::iced::Alignment::Center),
        );
    }
    let body: Element<'static, Message> = if any {
        strip.into()
    } else {
        row![
            icon_svg(
                severity_icon(Severity::Success),
                IconSize::Inline,
                severity_token(Severity::Success, &p).into_cosmic_color(),
            ),
            Space::new().width(Length::Fixed(6.0)),
            text("All clear")
                .size(12)
                .color(p.text_muted.into_cosmic_color()),
        ]
        .align_y(cosmic::iced::Alignment::Center)
        .into()
    };
    container(body)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .into()
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
    // One prefs load per frame (as the prior `hub_palette()` did), now also
    // yielding the density-scaled spacing + radii tokens the segmented tab bar
    // reads — so the new control carries no scattered metric literals (§4).
    let prefs = mde_theme::Preferences::load();
    let p = Palette::for_theme(prefs.theme);
    let space = mde_theme::Space::for_density(prefs.density);
    let radii = mde_theme::Radii::defaults();
    let now = now_ms();
    // NOTIFY-HUB-2 — single clock read for this frame's slide/blink sampling.
    let anim_now = std::time::Instant::now();

    // Header: the Carbon bell + title + "· N unread" + Close on one line; the
    // bulk actions (mark-all-read / clear-all) now live on the Notifications tab,
    // and the segmented tabs sit just below this header. Generous top/side
    // padding so nothing is jammed against the window edge.
    let unread = state.items.iter().filter(|i| !i.read).count();
    // NOTIFY-STATUS-STRIP — a wide, welcoming header indicator: the Carbon
    // notification bell (a real Material SVG, not a cryptic glyph) + the title in
    // heading size + the unread count as a muted live-state suffix, with generous
    // breathing room before the title.
    let title_row = row![
        icon_svg(
            Icon::Notification,
            IconSize::PanelHeader,
            p.accent.into_cosmic_color()
        ),
        Space::new().width(Length::Fixed(12.0)),
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
    // NOTIFY-REDESIGN-A — the segmented/pill tabs sit directly under the header.
    // The tab switches ONLY the scrollable content below; the status footer is
    // rendered outside it (see the sections vec), so it persists on both tabs.
    let tabs = tab_bar(state.tab, p, space, radii);

    // The active tab's scrollable content. Each tab owns its own actions (the
    // notifications mark-all/clear-all; the clipboard clear-all) and its own
    // honest empty state.
    let tab_content: Element<'_, Message> = match state.tab {
        Tab::Notifications => notifications_tab(&state.items, anim_now, &state.anim, p),
        Tab::Clipboard => clipboard_tab(&state.clips, now, p),
    };
    let scroll = scrollable(
        container(tab_content)
            .padding(Padding::from([12u16, 14u16]))
            .width(Length::Fill),
    );

    // Bottom quick-launch bar (W10 Action Center "quick actions" row): open the
    // Workbench, MDE-Files, or Cosmic Settings, then dismiss the panel.
    let launch_bar = container(
        row![
            launch_tile(Icon::Workbench, "Workbench", "mde-workbench", p),
            launch_tile(Icon::Files, "Files", "mde-files", p),
            launch_tile(Icon::Settings, "Settings", "cosmic-settings", p),
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
        // Header (Carbon bell + title + unread + Close) and the tabs stay pinned
        // at the top — they don't scroll with the list.
        column![title_row, tabs]
            .spacing(8)
            .padding(Padding::from([14u16, 16u16]))
            .into(),
        container(scroll).height(Length::Fill).into(),
        section_divider(p),
        // NOTIFY-REDESIGN-A — the ambient severity strip persists on both tabs.
        severity_strip(&state.items, p),
    ];
    // NOTIFY-REDESIGN-B — the Music popover (the compact, on-demand replacement for
    // the old full-width Now-Playing bar) appears just above the footer when its
    // icon is toggled open AND the music daemon is reachable.
    if state.music_popover && music_icon_state(state.music.as_ref()) != MusicIconState::Hidden {
        sections.push(music_popover_card(
            state.music.as_ref(),
            state.now_art.as_ref(),
            p,
        ));
    }
    // NOTIFY-REDESIGN-B — the combined status footer: "Lighthouses | Voice & Music",
    // two labeled sections side by side, each hiding when empty (its leading divider
    // drops with it). Persists on both tabs (rendered outside the scroll).
    if let Some(footer) = status_footer(
        &state.lighthouses,
        state.beam_step,
        state.music.as_ref(),
        state.voice.as_ref(),
        state.reduce_motion,
        p,
    ) {
        sections.push(section_divider(p));
        sections.push(footer);
    }
    sections.push(section_divider(p));
    sections.push(quick_actions.into());
    sections.push(launch_bar.into());

    // NOTIFY-FX-1 — fold the launcher's "menu effects" open-in into the Hub: the
    // whole content body starts a few px low and rises to rest on open (the shared
    // `panel_mount` slide-up `open_in` resolves — the identical vocabulary the
    // Application Menu's `menu_in()` uses). Rendered as decaying top padding (iced
    // 0.13 has no transform widget — MOTION-INFRA-2's translate-as-padding idiom,
    // exactly as the launcher applies it). The surface is full-height
    // (`Length::Fill`), so nudging the content down within it never reflows the
    // layer surface; at rest / under a disabled motion preference the offset is 0,
    // so the Hub renders unchanged once the open beat has elapsed.
    let slide = state.open_in(anim_now).translate_y.max(0.0);
    let content: Element<'_, Message> =
        container(cosmic::iced::widget::column(sections).spacing(0))
            .padding(Padding {
                top: slide,
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

/// NOTIFY-REDESIGN-B — the Music icon's now-playing **popover**: the compact card
/// the footer Music icon reveals on click. It carries the same album-art tile +
/// title/artist + transport (prev / play-pause / next) the removed full-width
/// Now-Playing bar did — REUSING the `MusicPrev`/`MusicToggle`/`MusicNext`
/// messages and the decoded cover-art handle — but only on demand, so the resting
/// footer stays a single compact icon. Honest idle / needs-setup states when
/// nothing is playing.
fn music_popover_card(
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
    // A raised, bordered card so the on-demand popover reads as a surface floating
    // above the footer rather than another inline section.
    container(body)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(cosmic::iced::Background::Color(
                p.raised.into_cosmic_color(),
            )),
            border: cosmic::iced::Border {
                color: p.border.into_cosmic_color(),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// NOTIFY-REDESIGN-B — the footer **Music** icon: a lighthouse-style square (the
/// `beacon_card` idiom — a bordered tile + a glyph in the state token + a label)
/// over a "Music" caption. The glyph gently PULSES (size breathe via the shared
/// `pulse_scale`, riding the beam clock) while a track is playing, is static
/// accent when paused, and greys when idle. Pressing it toggles the now-playing
/// popover. Never drawn when `Hidden` — the caller gates that.
fn music_icon_card(
    st: MusicIconState,
    beam_step: u16,
    reduce_motion: bool,
    p: Palette,
) -> Element<'static, Message> {
    let color = st.token(&p);
    // Pulse the glyph SIZE (the iced fork has no transform widget); a fixed square
    // hosts it so the breathe never reflows the row.
    let glyph_size = if st.pulses() && !reduce_motion {
        VM_ICON_GLYPH_PX
            * mde_theme::animation::pulse_scale(
                vm_phase(beam_step, mde_theme::Motion::notification_pulse().duration),
                mde_theme::PULSE_MAX_SCALE,
            )
    } else {
        VM_ICON_GLYPH_PX
    };
    vm_icon_card(
        text("\u{266A}")
            .size(glyph_size)
            .color(color.into_cosmic_color())
            .into(),
        color,
        "Music",
        Message::MusicTogglePopover,
        p,
    )
}

/// NOTIFY-REDESIGN-B — the footer **Voice** icon: the same lighthouse-style square
/// over a "Voice" caption. The glyph BLINKS (accent alpha breathe riding the beam
/// clock) while on a call, shows the ok-tone when registered/ready, and greys when
/// idle. A filled dot marks an active agent vs the hollow idle dot (a shape cue, so
/// state never rests on colour alone — the NOTIFY-STATUS-STRIP a11y rule). Pressing
/// it opens the Voice HUD (the existing detached-spawn launch idiom). Never drawn
/// when `Hidden`.
fn voice_icon_card(
    st: VoiceIconState,
    beam_step: u16,
    reduce_motion: bool,
    p: Palette,
) -> Element<'static, Message> {
    let base = st.token(&p);
    // Blink the glyph ALPHA (no opacity widget — the alpha rides the glyph colour);
    // the border keeps the steady token so only the glyph pulses.
    let glyph_color = if st.blinks() && !reduce_motion {
        base.with_alpha(blink_alpha(vm_phase(
            beam_step,
            mde_theme::Motion::loading().duration,
        )))
    } else {
        base
    };
    // Filled dot = a live/registered agent; hollow = idle (the shape cue).
    let glyph = if matches!(st, VoiceIconState::Idle) {
        "\u{25CB}" // ○
    } else {
        "\u{25CF}" // ●
    };
    vm_icon_card(
        text(glyph)
            .size(VM_ICON_GLYPH_PX)
            .color(glyph_color.into_cosmic_color())
            .into(),
        base,
        "Voice",
        Message::OpenApp("mde-voice-hud"),
        p,
    )
}

/// NOTIFY-REDESIGN-B — the shared shell for a Voice/Music icon card: a bordered
/// square (border in `border_color`, fill `p.surface`) hosting `glyph`, captioned
/// with `label`, pressing through to `on_press`. Mirrors `beacon_card` so the two
/// footer sections read as one family.
fn vm_icon_card(
    glyph: Element<'static, Message>,
    border_color: mde_theme::Rgba,
    label: &'static str,
    on_press: Message,
    p: Palette,
) -> Element<'static, Message> {
    let square = container(glyph)
        .center_x(Length::Fixed(VM_ICON_SQUARE_PX))
        .center_y(Length::Fixed(VM_ICON_SQUARE_PX))
        .style(move |_| container::Style {
            background: Some(cosmic::iced::Background::Color(
                p.surface.into_cosmic_color(),
            )),
            border: cosmic::iced::Border {
                color: border_color.into_cosmic_color(),
                width: 2.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        });
    let body = column![
        square,
        text(label).size(10).color(p.text_muted.into_cosmic_color()),
    ]
    .spacing(2)
    .align_x(cosmic::iced::Alignment::Center)
    .width(Length::Fixed(VM_ICON_CARD_PX));
    button(body)
        .on_press(on_press)
        .style(|_, _| button::Style {
            background: None,
            ..Default::default()
        })
        .padding(0)
        .into()
}

/// NOTIFY-REDESIGN-B — the "Voice & Music" labeled footer block: a heading (a
/// Carbon SVG via the reused `mde_icon` path + the title) over the two status
/// icons. Each icon hides independently when its own service is offline; the whole
/// block is `None` when BOTH are absent (mirrors the lighthouse footer's
/// hide-when-empty). Returned WITHOUT outer padding so [`status_footer`] can place
/// it beside the Lighthouses block.
fn voice_music_inner(
    music: Option<&MusicNow>,
    voice: Option<&VoiceStatus>,
    beam_step: u16,
    reduce_motion: bool,
    p: Palette,
) -> Option<Element<'static, Message>> {
    let m = music_icon_state(music);
    let v = voice_icon_state(voice);
    if m == MusicIconState::Hidden && v == VoiceIconState::Hidden {
        return None;
    }
    let header = row![
        icon_svg(Icon::Sound, IconSize::Inline, p.text.into_cosmic_color()),
        Space::new().width(Length::Fixed(8.0)),
        text("Voice & Music")
            .size(13)
            .color(p.text.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    let mut icons = row![].spacing(10).align_y(cosmic::iced::Alignment::Center);
    if v != VoiceIconState::Hidden {
        icons = icons.push(voice_icon_card(v, beam_step, reduce_motion, p));
    }
    if m != MusicIconState::Hidden {
        icons = icons.push(music_icon_card(m, beam_step, reduce_motion, p));
    }
    Some(
        column![header, Space::new().height(Length::Fixed(8.0)), icons]
            .spacing(0)
            .into(),
    )
}

/// LIGHTHOUSE-3 — the Lighthouses labeled block (header + the horizontally-
/// scrollable beacon strip), WITHOUT outer padding so [`status_footer`] can place
/// it beside the Voice & Music block. (Split out of the former `lighthouses_footer`
/// for the NOTIFY-REDESIGN-B side-by-side footer; the header/strip are unchanged.)
fn lighthouses_inner(beacons: &[Beacon], beam_step: u16, p: Palette) -> Element<'static, Message> {
    let (healthy, total) = lighthouse::health_counts(beacons);
    let count_color = if healthy == total {
        p.beacon_healthy
    } else {
        p.danger
    };
    let header = row![
        icon_svg(
            Icon::Firewall,
            IconSize::Inline,
            count_color.into_cosmic_color()
        ),
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

    column![header, Space::new().height(Length::Fixed(8.0)), strip,]
        .spacing(0)
        .into()
}

/// NOTIFY-REDESIGN-B — the combined bottom status footer: the Lighthouses block
/// and the new Voice & Music block rendered as two labeled sections SIDE BY SIDE
/// ("Lighthouses" | "Voice & Music"), each independently hiding when empty. The
/// whole footer is `None` when neither has anything to show, so the caller can also
/// drop its leading divider (the lighthouse-footer hide-when-empty convention,
/// extended to cover both). Persists on BOTH tabs (rendered outside the scroll).
fn status_footer(
    lighthouses: &[Beacon],
    beam_step: u16,
    music: Option<&MusicNow>,
    voice: Option<&VoiceStatus>,
    reduce_motion: bool,
    p: Palette,
) -> Option<Element<'static, Message>> {
    let lh = (!lighthouses.is_empty()).then(|| lighthouses_inner(lighthouses, beam_step, p));
    let vm = voice_music_inner(music, voice, beam_step, reduce_motion, p);
    let body: Element<'static, Message> = match (lh, vm) {
        (Some(lh), Some(vm)) => row![
            container(lh).width(Length::FillPortion(3)),
            vertical_divider(p),
            container(vm).width(Length::Shrink),
        ]
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Start)
        .into(),
        (Some(lh), None) => lh,
        (None, Some(vm)) => vm,
        (None, None) => return None,
    };
    Some(
        container(body)
            .padding(Padding::from([10u16, 14u16]))
            .width(Length::Fill)
            .into(),
    )
}

/// A thin vertical divider between the two side-by-side footer sections (the
/// vertical peer of [`section_divider`]).
fn vertical_divider(p: Palette) -> Element<'static, Message> {
    container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill))
        .style(move |_| container::Style {
            snap: false,
            background: Some(cosmic::iced::Background::Color(
                p.border.into_cosmic_color(),
            )),
            ..Default::default()
        })
        .into()
}

/// NOTIFY-REDESIGN-A — the Notifications tab body: the bulk actions (mark-all-
/// read / clear-all, kept working here on the Notifications tab) over the flat
/// newest-first message list, or an honest empty state. Each row is message-first
/// (severity icon + the message); the sender/host/age live in the later click-
/// opened detail viewer, so a row at rest is message-only (no hover subline).
fn notifications_tab(
    items: &[AlertItem],
    anim_now: std::time::Instant,
    anim: &HubAnim,
    p: Palette,
) -> Element<'static, Message> {
    let mut col = column![].spacing(8);
    if items.is_empty() {
        col = col.push(
            text("No alerts.")
                .size(13)
                .color(p.text_muted.into_cosmic_color()),
        );
        return col.into();
    }
    col = col.push(
        row![
            action_button("Mark all read", Message::MarkAllRead, p),
            Space::new().width(Length::Fixed(8.0)),
            action_button("Clear all", Message::ClearAll, p),
        ]
        .align_y(cosmic::iced::Alignment::Center),
    );
    // Flatten: one chronological, newest-first stream (no source groups, no
    // same-source stacking) — source identity is the future detail viewer's job.
    for (i, item) in flat_newest_first(items).iter().enumerate() {
        col = col.push(notification_row(item, i, anim_now, anim, p));
    }
    col.into()
}

/// NOTIFY-REDESIGN-A — one message-first notification row: the shape-distinct
/// severity icon + the message in prominent type, wrapped in the zebra-shaded
/// card with the per-item slide/blink entrance motion (the NOTIFY-HUB-2 motion is
/// kept; only the grouping/stacking is gone). The row is message-ONLY at rest —
/// no sender/host/age subline — and clicking it is a no-op for now (the click-
/// opened detail viewer is a separate later unit).
fn notification_row(
    item: &AlertItem,
    idx: usize,
    anim_now: std::time::Instant,
    anim: &HubAnim,
    p: Palette,
) -> Element<'static, Message> {
    let sev_color = severity_token(item.severity, &p).into_cosmic_color();
    // Unread reads at full strength; an acknowledged alert dims to muted.
    let msg_color = if item.read { p.text_muted } else { p.text }.into_cosmic_color();
    let head = row![
        icon_svg(severity_icon(item.severity), IconSize::Inline, sev_color),
        Space::new().width(Length::Fixed(8.0)),
        // Message-first: the alert message reads larger than the surrounding chrome.
        text(item.title.clone()).size(14).color(msg_color),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    motioned_card(
        head.into(),
        idx,
        item.severity,
        anim.card_motion(&item.id, anim_now),
        p,
    )
}

/// NOTIFY-REDESIGN-A — the Clipboard tab body: the mesh-global clipboard history
/// (pinned first, then the daemon's newest-first order), each a click-to-copy
/// `clip_row` with per-entry pin + delete, plus a mesh-wide Clear all. Moved out
/// of the cramped footer into its own scrollable tab, so it shows the FULL synced
/// history (no 6-row peek + "+N more"); it reuses the CLIP-VIEW-1 `clip_row`
/// rendering. Honest empty state.
///
/// `now_ms` is the frame clock in milliseconds — the same epoch-ms clock
/// [`format_age`] takes — so the clipboard ages read off the same bucket ladder.
fn clipboard_tab(clips: &[ClipRow], now_ms: i64, p: Palette) -> Element<'static, Message> {
    let mut col = column![].spacing(6);
    // Clear-all only when there is at least one unpinned entry to wipe (pinned
    // survive a clear, so an all-pinned history has nothing to clear).
    if clips.iter().any(|c| !c.pinned) {
        col = col.push(
            row![
                Space::new().width(Length::Fill),
                action_button("Clear all", Message::ClipClearAll, p),
            ]
            .align_y(cosmic::iced::Alignment::Center),
        );
    }
    if clips.is_empty() {
        col = col.push(
            text("Clipboard history is empty.")
                .size(12)
                .color(p.text_muted.into_cosmic_color()),
        );
        return col.into();
    }
    // Pinned first (the operator's kept clips), then the rest newest-first.
    let mut ordered: Vec<&ClipRow> = clips.iter().filter(|c| c.pinned).collect();
    ordered.extend(clips.iter().filter(|c| !c.pinned));
    for (i, c) in ordered.iter().enumerate() {
        col = col.push(clip_row(c, i, now_ms, p));
    }
    col.into()
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

/// A compact transport control button (the popover's prev / play-pause / next).
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

/// A bottom quick-launch tile: a Carbon icon over a centred label + the binary it
/// spawns (`OpenApp`). NOTIFY-STATUS-STRIP — the cryptic ⌗ / ▤ / ⚙ glyphs are now
/// real Material SVGs (`Workbench` / `Files` / `Settings`); the label is kept so
/// the tile is a labeled, welcoming launcher rather than glyph soup.
fn launch_tile<'a>(
    icon: Icon,
    label: &'a str,
    cmd: &'static str,
    p: Palette,
) -> Element<'a, Message> {
    button(
        cosmic::iced::widget::column![
            icon_svg(icon, IconSize::PanelHeader, p.text.into_cosmic_color()),
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

fn action_button<'a>(label: &'a str, msg: Message, p: Palette) -> Element<'a, Message> {
    button(text(label).size(12).color(p.text.into_cosmic_color()))
        .padding(Padding::from([4u16, 10u16]))
        .on_press(msg)
        .style(carbon_btn(p))
        .into()
}

/// NOTIFY-REDESIGN-A — the segmented/pill tab bar under the header: two equal-
/// width segments (Notifications / Clipboard) in a rounded track, the active one
/// filled in the accent. Every metric is an `mde-theme` token — `Space` (density-
/// scaled padding/gaps), `Radii::full` (the pill corners), `FontSize` (the label
/// size) — so the control carries no scattered literals (§4).
fn tab_bar(
    active: Tab,
    p: Palette,
    space: mde_theme::Space,
    radii: mde_theme::Radii,
) -> Element<'static, Message> {
    container(
        row![
            tab_segment(
                "Notifications",
                Tab::Notifications,
                active == Tab::Notifications,
                p,
                space,
                radii,
            ),
            tab_segment(
                "Clipboard",
                Tab::Clipboard,
                active == Tab::Clipboard,
                p,
                space,
                radii,
            ),
        ]
        .spacing(space.xs2),
    )
    .padding(Padding::from([space.xs2, space.xs2]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(cosmic::iced::Background::Color(
            p.surface.into_cosmic_color(),
        )),
        border: cosmic::iced::Border {
            color: p.border.into_cosmic_color(),
            width: 1.0,
            radius: f32::from(radii.full).into(),
        },
        ..Default::default()
    })
    .into()
}

/// NOTIFY-REDESIGN-A — one segment of [`tab_bar`]. Active = accent fill with
/// background-coloured (high-contrast) text; inactive = muted text on a
/// transparent segment that fills to `raised` on hover (the Application Menu's
/// subtle-button idiom). Both fill the track width so the two segments split it.
fn tab_segment(
    label: &'static str,
    tab: Tab,
    active: bool,
    p: Palette,
    space: mde_theme::Space,
    radii: mde_theme::Radii,
) -> Element<'static, Message> {
    let fg = if active { p.background } else { p.text_muted };
    button(
        container(
            text(label)
                .size(mde_theme::FontSize::defaults().body)
                .color(fg.into_cosmic_color()),
        )
        .center_x(Length::Fill)
        .padding(Padding::from([space.xs, space.sm])),
    )
    .width(Length::Fill)
    .on_press(Message::SwitchTab(tab))
    .style(move |_t, status| {
        use cosmic::iced::widget::button::Status;
        let bg = if active {
            Some(cosmic::iced::Background::Color(
                p.accent.into_cosmic_color(),
            ))
        } else if matches!(status, Status::Hovered | Status::Pressed) {
            Some(cosmic::iced::Background::Color(
                p.raised.into_cosmic_color(),
            ))
        } else {
            None
        };
        cosmic::iced::widget::button::Style {
            background: bg,
            text_color: fg.into_cosmic_color(),
            border: cosmic::iced::Border {
                color: cosmic::iced::Color::TRANSPARENT,
                width: 0.0,
                radius: f32::from(radii.full).into(),
            },
            ..Default::default()
        }
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    // `Source` is now only named in tests — the redesign's flat list dropped the
    // source-rank/grouping that used it in non-test code — so import it here.
    use mde_notify::Source;

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

    /// NOTIFY-REDESIGN-A — the Notifications tab is the default, and `SwitchTab`
    /// flips the live tab (the only thing it changes is which scrollable content
    /// renders; the status footer is unconditional).
    #[test]
    fn default_tab_is_notifications_and_switch_tab_flips_it() {
        let mut state = Center::new();
        assert_eq!(
            state.tab,
            Tab::Notifications,
            "Notifications is the default tab"
        );
        let _ = update(&mut state, Message::SwitchTab(Tab::Clipboard));
        assert_eq!(state.tab, Tab::Clipboard);
        let _ = update(&mut state, Message::SwitchTab(Tab::Notifications));
        assert_eq!(state.tab, Tab::Notifications);
    }

    /// NOTIFY-REDESIGN-A — the list is ONE flat chronological (newest-first)
    /// stream, not bucketed by source: two interleaved sources come back purely
    /// in timestamp order, proving the per-source grouping is gone.
    #[test]
    fn flat_newest_first_is_chronological_not_grouped_by_source() {
        let items = vec![
            item("a", Source::System, Severity::Info, 10),
            item("b", Source::Security, Severity::Critical, 40),
            item("c", Source::System, Severity::Info, 30),
            item("d", Source::Security, Severity::Warning, 20),
        ];
        let ids: Vec<String> = flat_newest_first(&items)
            .iter()
            .map(|i| i.id.clone())
            .collect();
        // Pure newest-first by ts (40,30,20,10); sources stay interleaved — a
        // grouped order would instead cluster the two Security rows together.
        assert_eq!(ids, ["b", "c", "d", "a"]);
    }

    /// NOTIFY-REDESIGN-A — both tab bodies build a renderable view without panic:
    /// the Notifications tab (a message-first row + the bulk actions + the
    /// severity strip) and the Clipboard tab (a clip row), each with real content.
    #[test]
    fn view_renders_on_both_tabs() {
        let mut state = Center::new();
        state.items.push(AlertItem {
            id: "x".into(),
            ts_unix_ms: 1,
            severity: Severity::Warning,
            source: Source::System,
            topic: "t".into(),
            host: Some("node".into()),
            title: "disk almost full".into(),
            body: "details".into(),
            read: false,
        });
        state.clips.push(ClipRow {
            id: "c1".into(),
            text: "hello".into(),
            source: "node".into(),
            time: String::new(),
            pinned: false,
        });
        // Notifications tab (default) builds.
        let _ = view(&state, window::Id::unique());
        // Clipboard tab builds.
        state.tab = Tab::Clipboard;
        let _ = view(&state, window::Id::unique());
    }

    /// NOTIFY-STATUS-STRIP — every severity maps to its shape-distinct Carbon
    /// status icon, and each icon resolves to a real Material SVG payload (the
    /// severity reads by icon SHAPE, never colour alone — kept by this redesign,
    /// in the rows and the severity strip).
    #[test]
    fn severity_maps_to_carbon_icon() {
        assert_eq!(severity_icon(Severity::Critical), Icon::StatusError);
        assert_eq!(severity_icon(Severity::Warning), Icon::StatusWarning);
        assert_eq!(severity_icon(Severity::Info), Icon::StatusInfo);
        assert_eq!(severity_icon(Severity::Success), Icon::StatusOk);

        for s in [
            Severity::Critical,
            Severity::Warning,
            Severity::Info,
            Severity::Success,
        ] {
            let bytes = mde_icon(severity_icon(s), IconSize::Inline).svg_bytes();
            assert!(
                bytes.is_some_and(|b| b.len() > 32),
                "severity {s:?} status icon must resolve a real Carbon SVG"
            );
        }
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
            state.voice.as_ref().map(|v| (v.registered, v.listening)),
            Some((true, true)),
            "voice snapshot not refetched inline by Refresh"
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

    /// NOTIFY-FX-1 — the Hub-open animation folds in the launcher's idiom: on open
    /// the content body is offset down by the shared `panel_mount` slide distance
    /// and eases up to rest, and the tick subscription stays armed only while that
    /// open beat is in flight (MOTION-PERF-1). We assert against the SAME shared
    /// helper the launcher's `menu_in()` resolves, so the two open with one
    /// vocabulary (not a Hub-local reimplementation).
    #[test]
    fn hub_open_in_matches_the_launcher_slide_idiom() {
        use std::time::Duration;
        let mut state = Center::new();
        // Pin the open clock so the assertions are deterministic.
        let t0 = std::time::Instant::now();
        state.opened_at = t0;
        state.reduce_motion = false;

        // At the open instant the body starts offset down by the shared
        // panel-mount slide distance — the exact launcher vocabulary.
        let at_open = state.open_in(t0);
        assert!(
            (at_open.translate_y - mde_theme::PANEL_MOUNT_TRANSLATE_Y_PX).abs() < 1e-3,
            "Hub opens offset by the shared panel-mount slide, got {}",
            at_open.translate_y
        );
        assert!(state.open_in_flight(t0), "the open beat is in flight at t0");

        // Past the panel-mount beat it has settled to rest (no offset) and the
        // open-in tick stops (so an open Hub at rest does no animation work).
        let beat = mde_theme::Motion::panel_mount().duration;
        let after = t0 + beat + Duration::from_millis(1);
        assert!(
            state.open_in(after).translate_y.abs() < 1e-3,
            "the open-in slide settles to rest after one panel-mount beat"
        );
        assert!(
            !state.open_in_flight(after),
            "the open beat is over, so the tick gate releases"
        );
    }

    /// NOTIFY-FX-1 — under reduce-motion the open-in collapses to a pure crossfade
    /// (no positional slide), honoring the same a11y contract the per-item motion
    /// already does — and the shared helper, not a Hub-local branch, enforces it.
    #[test]
    fn hub_open_in_has_no_slide_under_reduce_motion() {
        let mut state = Center::new();
        state.opened_at = std::time::Instant::now();
        state.reduce_motion = true;
        // Sampled anywhere in the window, the reduce-motion open-in never moves the
        // surface — translate_y is always 0 (opacity-only crossfade).
        let p = state.open_in(state.opened_at);
        assert!(
            p.translate_y.abs() < 1e-6,
            "reduce-motion open-in must not slide, got {}",
            p.translate_y
        );
    }

    /// NOTIFY-REDESIGN-B — the Music snapshot maps to the four icon render states:
    /// hidden when the daemon is absent, muted-idle when present-but-stopped,
    /// accent-paused with a loaded track, and accent-pulsing while playing.
    #[test]
    fn music_icon_state_maps_snapshot_to_render_state() {
        assert_eq!(music_icon_state(None), MusicIconState::Hidden);
        // ok:false / inactive → present but idle (greyed), never hidden.
        assert_eq!(
            music_icon_state(Some(&MusicNow::default())),
            MusicIconState::Idle
        );
        // needs-setup is still "present but idle" (a configure prompt, not offline).
        assert_eq!(
            music_icon_state(Some(&MusicNow {
                needs_airsonic: true,
                ..MusicNow::default()
            })),
            MusicIconState::Idle
        );
        assert_eq!(
            music_icon_state(Some(&MusicNow {
                active: true,
                playing: false,
                ..MusicNow::default()
            })),
            MusicIconState::Paused
        );
        assert_eq!(
            music_icon_state(Some(&MusicNow {
                active: true,
                playing: true,
                ..MusicNow::default()
            })),
            MusicIconState::Playing
        );
        // Only Playing animates (gentle pulse); paused/idle are static.
        assert!(MusicIconState::Playing.pulses());
        assert!(!MusicIconState::Paused.pulses());
        assert!(!MusicIconState::Idle.pulses());
    }

    /// NOTIFY-REDESIGN-B — the Voice snapshot maps to the four icon render states:
    /// hidden when stale/absent, muted-idle when unregistered, ok-tone when
    /// registered, and accent-blinking while on a call.
    #[test]
    fn voice_icon_state_maps_snapshot_to_render_state() {
        assert_eq!(voice_icon_state(None), VoiceIconState::Hidden);
        // A stale snapshot (agent offline) hides regardless of its last flags.
        assert_eq!(
            voice_icon_state(Some(&VoiceStatus {
                registered: true,
                listening: true,
                fresh: false,
            })),
            VoiceIconState::Hidden
        );
        assert_eq!(
            voice_icon_state(Some(&VoiceStatus {
                fresh: true,
                ..VoiceStatus::default()
            })),
            VoiceIconState::Idle
        );
        assert_eq!(
            voice_icon_state(Some(&VoiceStatus {
                fresh: true,
                registered: true,
                ..VoiceStatus::default()
            })),
            VoiceIconState::Ready
        );
        assert_eq!(
            voice_icon_state(Some(&VoiceStatus {
                fresh: true,
                registered: true,
                listening: true,
            })),
            VoiceIconState::InCall
        );
        // Only an in-call agent animates (blink); ready/idle are static.
        assert!(VoiceIconState::InCall.blinks());
        assert!(!VoiceIconState::Ready.blinks());
        assert!(!VoiceIconState::Idle.blinks());
    }

    /// NOTIFY-REDESIGN-B — the "Voice & Music" block hides only when BOTH services
    /// are absent and shows as soon as either is present (the lighthouse-footer
    /// hide-when-empty convention, applied to the pair).
    #[test]
    fn voice_music_section_hides_when_both_absent() {
        let p = hub_palette();
        // Both absent → no block.
        assert!(voice_music_inner(None, None, 0, false, p).is_none());
        // Music present (idle) → block shows.
        assert!(voice_music_inner(Some(&MusicNow::default()), None, 0, false, p).is_some());
        // Voice present (idle) → block shows.
        let v = VoiceStatus {
            fresh: true,
            ..VoiceStatus::default()
        };
        assert!(voice_music_inner(None, Some(&v), 0, false, p).is_some());
    }

    /// NOTIFY-REDESIGN-B — the combined footer renders iff there is at least one
    /// lighthouse OR a present Voice/Music service; with neither it is `None` so the
    /// caller drops the whole footer (and its divider).
    #[test]
    fn status_footer_shows_only_with_content() {
        let p = hub_palette();
        // Nothing at all → no footer.
        assert!(status_footer(&[], 0, None, None, false, p).is_none());
        // A lighthouse alone → footer.
        assert!(status_footer(&[beacon_sentinel()], 0, None, None, false, p).is_some());
        // Voice/Music alone → footer.
        assert!(status_footer(&[], 0, Some(&MusicNow::default()), None, false, p).is_some());
        // Both → footer (the side-by-side case).
        assert!(status_footer(
            &[beacon_sentinel()],
            0,
            Some(&MusicNow::default()),
            None,
            false,
            p
        )
        .is_some());
    }

    /// NOTIFY-REDESIGN-B — the shared beam-clock phase stays in `[0,1]` and advances
    /// with the beam step over a motion-token loop.
    #[test]
    fn vm_phase_is_bounded_and_advances() {
        let period = mde_theme::Motion::notification_pulse().duration;
        for step in 0..512u16 {
            let ph = vm_phase(step, period);
            assert!(
                (0.0..=1.0).contains(&ph),
                "phase {ph} out of range at step {step}"
            );
        }
        // Phase 0 at step 0; strictly grows over the first step (period ≫ tick).
        assert!(vm_phase(0, period).abs() < 1e-6);
        assert!(vm_phase(1, period) > vm_phase(0, period));
    }

    /// NOTIFY-REDESIGN-B — the in-call blink alpha breathes between the dim floor
    /// and full opacity (floor at the endpoints, ~1.0 at mid), never out of range.
    #[test]
    fn blink_alpha_breathes_between_floor_and_full() {
        assert!((blink_alpha(0.0) - VOICE_BLINK_MIN_ALPHA).abs() < 1e-3);
        assert!((blink_alpha(1.0) - VOICE_BLINK_MIN_ALPHA).abs() < 1e-3);
        assert!((blink_alpha(0.5) - 1.0).abs() < 1e-3);
        for i in 0..=20 {
            let a = blink_alpha(i as f32 / 20.0);
            assert!(
                (VOICE_BLINK_MIN_ALPHA - 1e-3..=1.0 + 1e-3).contains(&a),
                "alpha {a} out of [floor,1]"
            );
        }
    }

    /// NOTIFY-REDESIGN-B — clicking the Music icon toggles the now-playing popover
    /// open/closed (the compact replacement for the removed full-width bar).
    #[test]
    fn music_popover_toggles() {
        let mut state = Center::new();
        assert!(!state.music_popover, "popover starts closed");
        let _ = update(&mut state, Message::MusicTogglePopover);
        assert!(state.music_popover);
        let _ = update(&mut state, Message::MusicTogglePopover);
        assert!(!state.music_popover);
    }

    /// NOTIFY-REDESIGN-B — the footer + popover render paths build without panic: a
    /// playing track + a live call + a lighthouse (the side-by-side footer with both
    /// motion states active) and the open Music popover, in full motion AND under
    /// reduce-motion (static icons).
    #[test]
    fn view_renders_voice_music_footer_and_popover() {
        let mut state = Center::new();
        state.music = Some(MusicNow {
            active: true,
            playing: true,
            title: "Song".into(),
            artist: "Artist".into(),
            audio_available: true,
            ..MusicNow::default()
        });
        state.voice = Some(VoiceStatus {
            fresh: true,
            registered: true,
            listening: true,
        });
        state.lighthouses = vec![beacon_sentinel()];
        state.beam_step = 7;
        state.music_popover = true;
        let _ = view(&state, window::Id::unique());
        // Under reduce-motion the icons are static, but the view must still build.
        state.reduce_motion = true;
        let _ = view(&state, window::Id::unique());
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
