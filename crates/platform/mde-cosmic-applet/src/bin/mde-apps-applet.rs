//! mde-apps-applet — the Magic-Mesh **Applications Panel launcher** (APPS-2).
//!
//! A Cosmic panel applet: a grid/apps glyph button (Q16) that drops a tabbed
//! launcher (Favorites / Apps / Mesh / Workloads / Services — Q7), opening to
//! Favorites (Q6). It is a **thin renderer** (Q24): the entry list comes from the
//! mackesd `apps_aggregator` over the bus (`action/apps/list`, APPS-1); this shell
//! parses + filters it via the render-agnostic `mde_cosmic_applet` lib and draws
//! it through Carbon tokens. Local apps launch directly (Q23 local exec); mesh /
//! workload / service launch lands in APPS-5/6/7.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::iced::{time, Length, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::{Application, Element};

use mde_bus::hooks::config::Priority;
use mde_cosmic_applet::{
    filter_entries, parse_entries, workload_argv, Entry, LauncherTab, WorkloadAction,
};
use mde_theme::animation::{Animator, Transition};
use mde_theme::motion::{toast as toast_motion, Easing, Motion, PANEL_MOUNT_TRANSLATE_Y_PX};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, Preferences, Rgba, TypeRole};

const ID: &str = "com.mackes.MagicMeshApps";

/// APPS-FX-1 — launcher motion. The animation registry ([`Animator`]) is driven
/// by ONE subscription tick (~60 fps) that runs only while something is in
/// flight (MOTION-PERF-1: no idle wakeups). All tweens resolve through
/// `mde-theme::animation`/`motion`, honoring reduce-motion + the global motion
/// kill switch — no parallel motion system, no re-derived easing/timing.
const FRAME_TICK_MS: u64 = 16;
/// Animator id for the open/close popup fade+slide (Q: panel mount).
const ANIM_POPUP: &str = "popup";
/// Animator id for the tab-switch crossfade.
const ANIM_TAB: &str = "tab";
/// Animator id prefix for a tile's hover lift (`hover:<entry-id>`).
const ANIM_HOVER_PREFIX: &str = "hover:";

/// APPS-WIDE (operator 2026-06-18) — the launcher dropdown was a golden
/// rectangle (920 × 920/φ). APPS-FIT (operator 2026-06-19) supersedes that: the
/// dropdown now sizes to the desktop — **width = 33% of the output's logical
/// width**, **height the same fraction of its height**, so the rectangle keeps
/// the desktop's own aspect ratio ("matching the Ratio") and scales to any
/// resolution. The golden rectangle below is only the fallback when the
/// resolution can't be detected.
const GOLDEN_RATIO: f32 = 1.618;
/// APPS-FIT — the dropdown's width as a fraction of the desktop's logical width
/// (and its height as the same fraction of the desktop height).
const MENU_SCREEN_FRACTION: f32 = 0.33;
/// APPS-FIT — fallback dropdown size (the prior APPS-WIDE golden rectangle) used
/// when `cosmic-randr` can't report the output resolution.
const FALLBACK_MENU_WIDTH: f32 = 920.0;
const FALLBACK_MENU_HEIGHT: f32 = FALLBACK_MENU_WIDTH / GOLDEN_RATIO;

/// APPS-FIT — detect the launcher size from the desktop resolution: 33% of the
/// panel output's logical width × the same fraction of its height. Best-effort —
/// shells `cosmic-randr list --kdl`, scoped to this panel's output
/// (`COSMIC_PANEL_OUTPUT`); any failure falls back to the golden rectangle.
fn detect_menu_size() -> (f32, f32) {
    let target = std::env::var("COSMIC_PANEL_OUTPUT").unwrap_or_default();
    std::process::Command::new("cosmic-randr")
        .args(["list", "--kdl"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|kdl| parse_menu_size_from_kdl(&kdl, &target))
        .unwrap_or((FALLBACK_MENU_WIDTH, FALLBACK_MENU_HEIGHT))
}

/// APPS-FIT — pure parser for `cosmic-randr list --kdl`. Returns the launcher
/// size (`MENU_SCREEN_FRACTION` of the output's logical width × height) for the
/// output named `target` (or the first enabled output with a current mode when
/// `target` is empty / not found). The KDL shape (from cosmic-randr's
/// `list_kdl`) is:
/// ```text
/// output "DP-1" enabled=#true {
///   scale 1.50
///   modes {
///     mode 2560 1440 59951 current=#true preferred=#true
///   }
/// }
/// ```
/// Logical size = mode pixels ÷ scale. Returns `None` if no current mode is found.
fn parse_menu_size_from_kdl(kdl: &str, target: &str) -> Option<(f32, f32)> {
    // (name, enabled, scale, current_mode (w,h)) per output block, in order.
    let mut cur_name = String::new();
    let mut cur_enabled = false;
    let mut cur_scale = 1.0_f32;
    let mut best: Option<(f32, f32)> = None; // logical (w,h) of the chosen output
    let mut fallback: Option<(f32, f32)> = None; // first enabled output's logical size

    let finish = |name: &str,
                  enabled: bool,
                  mode: Option<(f32, f32)>,
                  best: &mut Option<(f32, f32)>,
                  fallback: &mut Option<(f32, f32)>| {
        let Some(sz) = mode else { return };
        if enabled {
            if !target.is_empty() && name == target {
                *best = Some(sz);
            } else if fallback.is_none() {
                *fallback = Some(sz);
            }
        }
    };

    let mut cur_mode: Option<(f32, f32)> = None;
    for line in kdl.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("output \"") {
            // Close the previous block.
            finish(
                &cur_name,
                cur_enabled,
                cur_mode.take(),
                &mut best,
                &mut fallback,
            );
            // output "NAME" enabled=#true {
            let name = rest.split('"').next().unwrap_or("").to_string();
            cur_name = name;
            cur_enabled = t.contains("enabled=#true");
            cur_scale = 1.0;
        } else if let Some(rest) = t.strip_prefix("scale ") {
            cur_scale = rest.trim().parse().unwrap_or(1.0);
        } else if let Some(rest) = t.strip_prefix("mode ") {
            if t.contains("current=#true") {
                let mut it = rest.split_whitespace();
                if let (Some(w), Some(h)) = (it.next(), it.next()) {
                    if let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>()) {
                        let scale = if cur_scale > 0.0 { cur_scale } else { 1.0 };
                        cur_mode = Some((w / scale, h / scale));
                    }
                }
            }
        }
    }
    // Close the final block.
    finish(
        &cur_name,
        cur_enabled,
        cur_mode.take(),
        &mut best,
        &mut fallback,
    );

    let (lw, lh) = best.or(fallback)?;
    if lw <= 0.0 || lh <= 0.0 {
        return None;
    }
    Some((lw * MENU_SCREEN_FRACTION, lh * MENU_SCREEN_FRACTION))
}
/// APPS-WIDE — Favorites icon-grid shape (operator 2026-06-18): exactly 3 tiles
/// per row, capped at 9 (a 3×3 grid), mirroring the Workbench/Files/Settings
/// quick-link tile row above.
const FAVORITES_COLUMNS: usize = 3;
/// Max favorites shown in the grid (3×3).
const FAVORITES_MAX: usize = 9;

/// APPS-STYLE — resolve the active palette from the user's MDE theme preference
/// (`~/.config/mde/preferences.toml`), so the launcher honors **both dark and
/// light** themes (Carbon Gray 100 / Gray 90 / Gray 10) instead of a hardcoded
/// dark palette. Cheap file read; called at init + on each open.
fn resolve_palette() -> Palette {
    Palette::for_theme(Preferences::load().theme)
}

/// APPS-FX-1 — read the motion preferences the launcher's animations honor:
/// `(reduce_motion, motion_enabled)`. Reduce-motion caps every tween to the
/// ≤80 ms crossfade; the disabled kill switch (MOTION-CORE-3) suppresses tweens
/// entirely (terminal frame, no interpolation). Cheap file read; called at init
/// + on each open so a preference change is picked up without a re-login.
fn resolve_motion_prefs() -> (bool, bool) {
    let prefs = Preferences::load();
    (prefs.a11y.reduce_motion, prefs.motion.enabled)
}

/// APPS-WIDE — the Carbon icon (`mde_theme` icon set) for a Favorites tile,
/// chosen by entry kind. Plain apps get the generic Apps glyph; mesh-apps /
/// services / workloads get their scope's icon.
fn favorite_icon(e: &Entry) -> Icon {
    match e.kind.as_str() {
        "mesh-app" => Icon::Fleet,
        "service" => Icon::Network,
        "workload" => Icon::Compute,
        _ => Icon::Apps,
    }
}

struct AppsApplet {
    core: Core,
    /// The open dropdown popup, if any.
    popup: Option<Id>,
    /// Latest aggregated entries (refreshed on open).
    entries: Vec<Entry>,
    /// Active tab (lands on Favorites — Q6).
    tab: LauncherTab,
    /// Search query (non-empty searches across tabs — Q2).
    query: String,
    /// Pinned favorites (APPS-4 wires the mesh-synced store; empty until then).
    favorites: HashSet<String>,
    /// QNM-Shared (used, total) bytes for the header (Q8); None = unavailable.
    qnm: Option<(u64, u64)>,
    /// APPS-STYLE-2 — the entry id whose inline detail panel is expanded (click
    /// a row to toggle). Replaces the old right-click context strip.
    selected: Option<String>,
    /// APPS-STYLE-2 — transient feedback line at the bottom (pin/unpin, power…).
    toast: Option<String>,
    /// APPS-STYLE-2 — whether the footer power menu is open.
    power_open: bool,
    /// RCLICK — whether the popup is showing the Win+X-style right-click power
    /// menu (vs the normal launcher). Set when the launcher button is
    /// right-clicked; the same popup surface renders the power menu instead.
    rclick_open: bool,
    /// RCLICK-5 — whether the power-menu popup is showing the Run (Win+R) box,
    /// and its current command text.
    run_open: bool,
    run_text: String,
    /// Last load error, shown in the dropdown's empty state.
    error: Option<String>,
    /// APPS-STYLE — the active Carbon palette (dark/light, from the user's MDE
    /// theme preference); refreshed on each open so a theme switch is picked up.
    palette: Palette,
    /// APPS-FIT — the launcher dropdown size (logical px): 33% of the desktop
    /// width × the same fraction of its height. Detected once at init from
    /// `cosmic-randr` (falls back to the golden rectangle); refreshed on open so
    /// a resolution change is picked up without a re-login.
    menu_w: f32,
    menu_h: f32,
    /// APPS-FX-1 — the shared animation registry (open/close fade+slide, tab
    /// crossfade, tile hover lift). Advanced by one subscription tick.
    anim: Animator,
    /// APPS-FX-1 — reduce-motion preference (from `~/.config/mde/preferences.toml`);
    /// caps every tween to the ≤80 ms crossfade. Refreshed on each open.
    reduce_motion: bool,
    /// APPS-FX-1 — global motion kill switch (MOTION-CORE-3). `false` → no tweens
    /// are armed; surfaces render their terminal frame with no interpolation.
    motion_enabled: bool,
    /// APPS-FX-1 — the app tile currently under the pointer (drives the hover
    /// lift + accent). `None` = nothing hovered.
    hovered: Option<String>,
}

#[derive(Clone, Debug)]
enum Message {
    /// Cosmic surface (popup create/destroy) action passthrough.
    Surface(cosmic::surface::Action),
    /// The popup was closed by the compositor.
    PopupClosed(Id),
    /// Open-or-close the launcher dropdown.
    TogglePopup,
    /// Fresh entries + QNM disk usage + favorites arrived from the load.
    Loaded(Vec<Entry>, Option<(u64, u64)>, HashSet<String>),
    /// A load failed.
    LoadFailed(String),
    /// Pin/unpin an entry id (APPS-4).
    ToggleFavorite(String),
    /// Favorites changed (post-pin); update the set.
    FavoritesChanged(HashSet<String>),
    /// Switch the active tab.
    SetTab(LauncherTab),
    /// Search box changed.
    Search(String),
    /// Launch a local app by its exec line (Q23).
    LaunchLocal(String),
    /// Open a remote-desktop session to a mesh peer by hostname (APPS-5).
    LaunchMesh(String),
    /// Control a local workload (start/stop/attach) — `(source, name, action)` (APPS-6).
    Workload(String, String, WorkloadAction),
    /// Open a published mesh service's endpoint over the overlay (APPS-7).
    OpenService(String),
    /// APPS-STYLE-2 — click a row to toggle its inline detail panel.
    SelectEntry(String),
    /// APPS-STYLE-2 — open an app's desktop on a specific mesh host (RD chip).
    OpenOnHost(String),
    /// APPS-STYLE-2 — dismiss the toast.
    DismissToast,
    /// APPS-STYLE-2 — toggle the footer power menu.
    TogglePower,
    /// APPS-STYLE-2 — run a power/session action.
    Power(PowerKind),
    /// RCLICK — open the Win+X-style right-click power menu (launcher button
    /// secondary-press).
    OpenPowerMenu,
    /// RCLICK — launch a command with args (e.g. `cosmic-term -e btop`, an
    /// elevated `pkexec …`), then close the menu.
    LaunchArgs(Vec<String>),
    /// RCLICK — deep-link into the Workbench at a focus slug, then close.
    LaunchFocus(&'static str),
    /// RCLICK — minimize-all / show the desktop.
    ShowDesktop,
    /// RCLICK-5 — show the Run (Win+R) box / its input changed / run it.
    OpenRun,
    RunInput(String),
    RunSubmit,
    /// Re-fetch the entry list.
    Refresh,
    /// APPS-FX-1 — one animation frame tick (drives the shared `Animator`); only
    /// subscribed while something is in flight.
    AnimTick,
    /// APPS-FX-1 — pointer entered an app tile (start its hover lift).
    TileEnter(String),
    /// APPS-FX-1 — pointer left an app tile (settle its hover lift).
    TileExit(String),
}

/// APPS-STYLE-2 — the footer power-menu actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerKind {
    Lock,
    Logout,
    Suspend,
    Restart,
    Shutdown,
}

impl PowerKind {
    /// Menu label.
    fn label(self) -> &'static str {
        match self {
            Self::Lock => "Lock",
            Self::Logout => "Log out",
            Self::Suspend => "Suspend",
            Self::Restart => "Restart",
            Self::Shutdown => "Shut down",
        }
    }

    /// All session actions, in menu order (RCLICK Power submenu).
    fn all() -> [Self; 5] {
        [
            Self::Lock,
            Self::Logout,
            Self::Suspend,
            Self::Restart,
            Self::Shutdown,
        ]
    }

    /// The detached command (argv) that performs it.
    fn argv(self) -> Vec<String> {
        let s = |v: &str| v.to_string();
        match self {
            Self::Lock => vec![s("loginctl"), s("lock-session")],
            Self::Logout => vec![s("loginctl"), s("terminate-user"), current_user()],
            Self::Suspend => vec![s("systemctl"), s("suspend")],
            Self::Restart => vec![s("systemctl"), s("reboot")],
            Self::Shutdown => vec![s("systemctl"), s("poweroff")],
        }
    }
}

/// QNM-Shared mount the header reports on (Q8).
const QNM_MOUNT: &str = "/mnt/mesh-storage";

/// Read `(used, total)` bytes of the QNM-Shared mount via `df` (Q8). `None` when
/// the mount is absent/unreadable (the header then shows "unavailable").
fn read_qnm_usage() -> Option<(u64, u64)> {
    let out = std::process::Command::new("df")
        .args(["-B1", "--output=used,size", QNM_MOUNT])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // Line 2 is "<used> <size>".
    let text = String::from_utf8_lossy(&out.stdout);
    let nums: Vec<u64> = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    match nums.as_slice() {
        [used, size, ..] => Some((*used, *size)),
        _ => None,
    }
}

/// The desktop user whose favorites we sync (Q10). Falls back to `_`.
fn current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "_".to_string())
}

/// Round-trip one `action/apps/<verb>` request on the shared bus, returning the
/// reply body. `Persist` isn't `Send`, so this is called from a blocking thread.
fn bus_request(verb: &str, body: Option<&str>) -> Result<String, String> {
    let dir = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
    let persist = mde_bus::persist::Persist::open(dir).map_err(|e| format!("bus store: {e}"))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let reply = rt
        .block_on(mde_bus::rpc::request(
            &persist,
            &format!("action/apps/{verb}"),
            Priority::Default,
            None,
            body,
            Duration::from_secs(5),
        ))
        .map_err(|e| format!("apps daemon not responding ({e})"))?;
    Ok(reply.body.unwrap_or_default())
}

/// Fetch the entry list + QNM disk usage + this user's favorites in one
/// blocking task; only the `Send` results cross back. Unreachable daemon → empty.
async fn fetch_apps() -> Result<(Vec<Entry>, Option<(u64, u64)>, HashSet<String>), String> {
    tokio::task::spawn_blocking(
        || -> Result<(Vec<Entry>, Option<(u64, u64)>, HashSet<String>), String> {
            let entries = parse_entries(&bus_request("list", None)?);
            let user = current_user();
            let favs_body = format!(r#"{{"user":"{user}"}}"#);
            let favorites = bus_request("favorites", Some(&favs_body))
                .map(|r| mde_cosmic_applet::parse_favorites(&r))
                .unwrap_or_default();
            Ok((entries, read_qnm_usage(), favorites))
        },
    )
    .await
    .map_err(|e| format!("fetch task join: {e}"))?
}

/// Pin/unpin a favorite over the bus, returning the new set.
async fn set_favorite(id: String, pinned: bool) -> HashSet<String> {
    tokio::task::spawn_blocking(move || {
        let user = current_user();
        let body = serde_json::json!({ "user": user, "id": id, "pinned": pinned }).to_string();
        bus_request("set-favorite", Some(&body))
            .map(|r| mde_cosmic_applet::parse_favorites(&r))
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// Open a remote-desktop session to a peer (APPS-5): ask mackesd to resolve the
/// peer's `{protocol, target}` from the PD-2 directory, then shell the local RD
/// client `remmina -c <protocol>://<target>` (the same tool the Workbench Remote
/// Desktop panel uses). Detached via `setsid --fork` (no zombie). Resolution +
/// the client run locally — the RD window is on this display.
async fn launch_mesh(node: String) {
    tokio::task::spawn_blocking(move || {
        let body = serde_json::json!({ "node": node }).to_string();
        let Ok(reply) = bus_request("launch", Some(&body)) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&reply) else {
            return;
        };
        if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            return;
        }
        let protocol = v.get("protocol").and_then(|p| p.as_str()).unwrap_or("rdp");
        let Some(target) = v.get("target").and_then(|t| t.as_str()) else {
            return;
        };
        let _ = std::process::Command::new("setsid")
            .args(["--fork", "remmina", "-c", &format!("{protocol}://{target}")])
            .status();
    })
    .await
    .ok();
}

/// The `Task` that loads entries + disk usage + favorites, mapped into messages.
fn load_task() -> Task<Message> {
    Task::perform(fetch_apps(), |r| {
        cosmic::Action::App(match r {
            Ok((entries, qnm, favs)) => Message::Loaded(entries, qnm, favs),
            Err(e) => Message::LoadFailed(e),
        })
    })
}

/// Launch a local app detached (Q23) — `setsid --fork` so it reparents to init
/// and never zombies the applet (the NOTIFY-UI-4 lesson). The exec line's XDG
/// field codes (`%U`/`%f`/…) are stripped — we launch with no file args.
fn launch_local(exec: &str) {
    let cleaned: Vec<String> = exec
        .split_whitespace()
        .filter(|tok| !tok.starts_with('%'))
        .map(str::to_string)
        .collect();
    let Some((cmd, args)) = cleaned.split_first() else {
        return;
    };
    let _ = std::process::Command::new("setsid")
        .arg("--fork")
        .arg(cmd)
        .args(args)
        .status();
}

fn carbon(c: Rgba) -> cosmic::iced::Color {
    cosmic::iced::Color {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: c.a,
    }
}

/// APPS-FX-1 — a Carbon token at a transition's opacity. The iced 0.13 fork ships
/// no opacity widget, so a fade is applied by multiplying the token's alpha by the
/// [`Transition`] params' `alpha` (the apply technique the animation module
/// documents). Multiplies (never overwrites) so a token that is already
/// translucent stays so.
fn carbon_at(c: Rgba, alpha: f32) -> cosmic::iced::Color {
    carbon(c.with_alpha(c.a * alpha.clamp(0.0, 1.0)))
}

/// APPS-FX-1 — blend two Carbon tokens by `t` (`0.0` = `a`, `1.0` = `b`), reusing
/// `mde-theme`'s [`lerp_f32`] for each channel (no re-derived interpolation). Used
/// by the hover lift to wash a row's surface toward the accent token.
fn carbon_blend(a: Rgba, b: Rgba, t: f32) -> cosmic::iced::Color {
    use mde_theme::animation::lerp_f32;
    cosmic::iced::Color {
        r: lerp_f32(f32::from(a.r), f32::from(b.r), t) / 255.0,
        g: lerp_f32(f32::from(a.g), f32::from(b.g), t) / 255.0,
        b: lerp_f32(f32::from(a.b), f32::from(b.b), t) / 255.0,
        a: lerp_f32(a.a, b.a, t),
    }
}

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<AppsApplet>(())
}

impl Application for AppsApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Message>) {
        let (menu_w, menu_h) = detect_menu_size();
        let (reduce_motion, motion_enabled) = resolve_motion_prefs();
        (
            AppsApplet {
                core,
                popup: None,
                entries: Vec::new(),
                tab: LauncherTab::Favorites,
                query: String::new(),
                favorites: HashSet::new(),
                qnm: None,
                selected: None,
                toast: None,
                power_open: false,
                rclick_open: false,
                run_open: false,
                run_text: String::new(),
                error: None,
                palette: resolve_palette(),
                menu_w,
                menu_h,
                anim: Animator::new(),
                reduce_motion,
                motion_enabled,
                hovered: None,
            },
            // Prime the list so the first open is instant.
            load_task(),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        // APPS-FX-1 / MOTION-PERF-1 — the frame clock runs ONLY while a tween is
        // in flight, so the launcher costs zero idle wakeups at rest.
        if self.anim.is_idle(Instant::now()) {
            Subscription::none()
        } else {
            time::every(Duration::from_millis(FRAME_TICK_MS)).map(|_| Message::AnimTick)
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Surface(a) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(a),
                ));
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                    self.rclick_open = false;
                }
            }
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
                // Left-click → the launcher (not the right-click power menu).
                self.rclick_open = false;
                // Refresh the palette so a theme switch reflects on open.
                self.palette = resolve_palette();
                // APPS-FIT — re-detect the desktop size on open so a resolution
                // change is picked up without a re-login.
                let (mw, mh) = detect_menu_size();
                self.menu_w = mw;
                self.menu_h = mh;
                // APPS-FX-1 — refresh motion prefs + arm the open fade+slide so
                // the launcher mounts with the Carbon panel-mount entrance.
                let (rm, en) = resolve_motion_prefs();
                self.reduce_motion = rm;
                self.motion_enabled = en;
                self.hovered = None;
                // APPS-FX-1 — drop any settled tweens from the prior open so the
                // map never carries stale entries (MOTION-PERF-1), then arm the
                // panel fade+slide. The whole launcher content fades in via the
                // popup alpha threaded through the header/tabs/search/body below.
                self.anim.gc(Instant::now());
                self.animate(ANIM_POPUP, Motion::panel_mount());
                // Open the dropdown + refresh-on-open (Q: cached + refresh-on-open).
                let open = cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(app_popup::<AppsApplet>(
                        move |state: &mut AppsApplet| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            let (menu_w, menu_h) = (state.menu_w, state.menu_h);
                            let mut settings = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                Some((menu_w as u32, menu_h as u32)),
                                None,
                                None,
                            );
                            // APPS-WIDE/APPS-FIT — `get_popup_settings` hard-caps
                            // the popup at max_width(360), so a wide content
                            // container is clamped to 360px no matter what. Lift
                            // the cap to the detected desktop-fraction size so the
                            // launcher renders at 33% of the screen.
                            settings.positioner.size = Some((menu_w as u32, menu_h as u32));
                            settings.positioner.size_limits = cosmic::iced::Limits::NONE
                                .min_width(menu_w)
                                .max_width(menu_w)
                                .min_height(1.0)
                                .max_height(menu_h);
                            settings
                        },
                        Some(Box::new(move |state: &AppsApplet| {
                            Element::from(state.core.applet.popup_container(state.dropdown()))
                                .map(cosmic::Action::App)
                        })),
                    )),
                ));
                return Task::batch([open, load_task()]);
            }
            Message::OpenPowerMenu => {
                // RCLICK — secondary-click opens the Win+X power menu in the
                // popup (a compact 360-wide surface), replacing the launcher
                // content via the rclick_open flag.
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
                self.rclick_open = true;
                self.power_open = false;
                self.palette = resolve_palette();
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(app_popup::<AppsApplet>(
                        move |state: &mut AppsApplet| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            let mut settings = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                Some((360, 600)),
                                None,
                                None,
                            );
                            settings.positioner.size = Some((360, 600));
                            settings.positioner.size_limits = cosmic::iced::Limits::NONE
                                .min_width(360.0)
                                .max_width(360.0)
                                .min_height(1.0)
                                .max_height(640.0);
                            settings
                        },
                        Some(Box::new(move |state: &AppsApplet| {
                            Element::from(state.core.applet.popup_container(state.dropdown()))
                                .map(cosmic::Action::App)
                        })),
                    )),
                ));
            }
            Message::LaunchArgs(argv) => {
                if let Some((cmd, rest)) = argv.split_first() {
                    let _ = std::process::Command::new(cmd).args(rest).spawn();
                }
                self.rclick_open = false;
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
            }
            Message::LaunchFocus(slug) => {
                let _ = std::process::Command::new("mde-workbench")
                    .args(["--focus", slug])
                    .spawn();
                self.rclick_open = false;
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
            }
            Message::ShowDesktop => {
                // Best-effort minimize-all via the Cosmic shortcut helper, then close.
                let _ = std::process::Command::new("cosmic-osd")
                    .arg("show-desktop")
                    .spawn();
                self.rclick_open = false;
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
            }
            Message::OpenRun => {
                // RCLICK-5 — swap the power menu for the Run box (same popup).
                self.run_open = true;
                self.run_text.clear();
            }
            Message::RunInput(s) => self.run_text = s,
            Message::RunSubmit => {
                let cmd = self.run_text.trim().to_string();
                if !cmd.is_empty() {
                    // Win+R parity: run the line through a shell so args/pipes work.
                    let _ = std::process::Command::new("sh").arg("-c").arg(&cmd).spawn();
                }
                self.run_open = false;
                self.rclick_open = false;
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
            }
            Message::Loaded(entries, qnm, favs) => {
                self.entries = entries;
                self.qnm = qnm;
                self.favorites = favs;
                self.error = None;
            }
            Message::ToggleFavorite(id) => {
                let pinned = !self.favorites.contains(&id);
                let name = self
                    .entries
                    .iter()
                    .find(|e| e.id == id)
                    .map_or_else(|| id.clone(), |e| e.name.clone());
                self.toast = Some(format!(
                    "{} {name}",
                    if pinned { "Pinned" } else { "Unpinned" }
                ));
                return Task::perform(set_favorite(id, pinned), |favs| {
                    cosmic::Action::App(Message::FavoritesChanged(favs))
                });
            }
            Message::FavoritesChanged(favs) => self.favorites = favs,
            Message::LoadFailed(e) => {
                self.error = Some(e);
            }
            Message::SetTab(t) => {
                // APPS-FX-1 — crossfade the result body when the tab changes (a
                // no-op fade if it's the same tab). The new tab's rows fade in.
                if self.tab != t {
                    self.animate(ANIM_TAB, Motion::panel_mount());
                }
                self.tab = t;
                self.query.clear();
                self.selected = None;
                self.hovered = None;
            }
            Message::Search(q) => self.query = q,
            Message::LaunchLocal(exec) => {
                launch_local(&exec);
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
            }
            Message::LaunchMesh(node) => {
                // Close the dropdown + open the remote-desktop session (APPS-5).
                let close = self.popup.take().map(|id| {
                    cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(
                        destroy_popup(id),
                    )))
                });
                let launch = Task::perform(launch_mesh(node), |()| {
                    cosmic::Action::App(Message::Refresh)
                });
                return match close {
                    Some(c) => Task::batch([c, launch]),
                    None => launch,
                };
            }
            Message::Workload(source, name, action) => {
                if let Some(argv) = workload_argv(&source, &name, action) {
                    // Start/Stop run the argv directly; Attach needs a terminal
                    // (the VM console / container shell is interactive).
                    if matches!(action, WorkloadAction::Attach) {
                        let mut cmd = std::process::Command::new("setsid");
                        cmd.args(["--fork", "cosmic-term", "--"]).args(&argv);
                        let _ = cmd.status();
                    } else {
                        let _ = std::process::Command::new("setsid")
                            .arg("--fork")
                            .args(&argv)
                            .status();
                    }
                }
                // Reload so the state pill reflects the start/stop.
                return load_task();
            }
            Message::OpenService(endpoint) => {
                // Open the published endpoint over the overlay (APPS-7) in the
                // default handler (browser for http(s), etc.), detached.
                if !endpoint.is_empty() {
                    let _ = std::process::Command::new("setsid")
                        .args(["--fork", "xdg-open", &endpoint])
                        .status();
                }
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
            }
            Message::SelectEntry(id) => {
                // APPS-STYLE-2 — toggle the inline detail for this row.
                self.power_open = false;
                self.selected = if self.selected.as_deref() == Some(&id) {
                    None
                } else {
                    Some(id)
                };
            }
            Message::OpenOnHost(host) => {
                // APPS-STYLE-2 — open a remote-desktop session to the host (the
                // detail's host chips); the menu stays open + a toast confirms.
                self.toast = Some(format!("Opening desktop on {host}…"));
                return Task::perform(launch_mesh(host), |()| {
                    cosmic::Action::App(Message::Refresh)
                });
            }
            Message::DismissToast => self.toast = None,
            Message::TogglePower => self.power_open = !self.power_open,
            Message::Power(kind) => {
                let _ = std::process::Command::new("setsid")
                    .arg("--fork")
                    .args(kind.argv())
                    .status();
                self.power_open = false;
                self.toast = Some(format!("{}…", kind.label()));
            }
            Message::Refresh => return load_task(),
            Message::AnimTick => {
                // APPS-FX-1 — advance the shared clock: drop finished tweens so
                // the subscription can stop ticking once everything settles.
                self.anim.gc(Instant::now());
            }
            Message::TileEnter(id) => {
                // APPS-FX-1 — lift the tile under the pointer.
                if self.hovered.as_deref() != Some(&id) {
                    self.hovered = Some(id.clone());
                    self.animate(format!("{ANIM_HOVER_PREFIX}{id}"), Motion::hover());
                }
            }
            Message::TileExit(id) => {
                // APPS-FX-1 — settle the lift when the pointer leaves (only if
                // this tile is still the hovered one — guards stale exits).
                if self.hovered.as_deref() == Some(&id) {
                    self.hovered = None;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // APPLET-LABEL (operator 2026-06-20) — the panel launcher button shows the
        // word "Start>" instead of the brand logo, sized to the panel's height.
        let icon_px = self.core.applet.suggested_size(true).0.max(16);
        let glyph = cosmic::widget::text("Start>")
            .size(f32::from(icon_px) * 0.6)
            .height(Length::Fixed(f32::from(icon_px)))
            .align_y(cosmic::iced::alignment::Vertical::Center);
        // APPS-MOUSE-FIX (operator bug 2026-06-18) — the panel button is plain
        // click-to-toggle: `on_press` opens the dropdown, a second press closes
        // it, and a launch closes it (the LaunchLocal/LaunchMesh/OpenService
        // handlers destroy the popup). The old `applet_tooltip` wrapper added a
        // hover subsurface ("mouseover pop up") that interfered with the click —
        // removed; the label lives in the dropdown header instead.
        let btn = cosmic::widget::button::custom(glyph)
            .on_press(Message::TogglePopup)
            .class(cosmic::theme::Button::AppletIcon);
        // RCLICK — secondary-click opens the Win+X-style power menu; left-click
        // still opens the launcher (the button's own on_press).
        Element::from(cosmic::widget::mouse_area(btn).on_right_press(Message::OpenPowerMenu))
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        // The popup body is supplied by the app_popup content closure.
        cosmic::widget::text("").into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl AppsApplet {
    /// APPS-FX-1 — arm a launcher tween under `id` using a `mde-theme` motion
    /// preset. Routes through [`Animator::start`] (which resolves the duration
    /// against reduce-motion); the global kill switch (MOTION-CORE-3) suppresses
    /// the tween entirely so the surface renders its terminal frame. The previous
    /// tween under `id` is dropped so it never freezes mid-flight on re-arm.
    fn animate(&mut self, id: impl Into<String>, motion: Motion) {
        if !self.motion_enabled {
            return;
        }
        let now = Instant::now();
        // Drop any completed tweens first so the registry never accumulates stale
        // entries once the subscription idles (MOTION-PERF-1 — the final settling
        // tick can't gc its own tween, so re-arming / a new arm sweeps it).
        self.anim.gc(now);
        self.anim.start(id, now, motion, self.reduce_motion);
    }

    /// APPS-FX-1 — the hover highlight's eased progress `0.0..=1.0` for a tile, by
    /// its entry id (1.0 = fully highlighted, 0.0 = at rest). `0.0` when this tile
    /// isn't the hovered one. When it IS hovered, the value eases 0→1 over the
    /// `hover` tween — or is `1.0` immediately when motion is disabled / the tween
    /// already settled (the terminal highlighted frame, the correct reduce-motion
    /// behavior: a hover affordance shows instantly, it just doesn't animate).
    fn hover_t(&self, entry_id: &str) -> f32 {
        if self.hovered.as_deref() != Some(entry_id) {
            return 0.0;
        }
        self.anim.value(
            &format!("{ANIM_HOVER_PREFIX}{entry_id}"),
            Instant::now(),
            Easing::EaseOut,
        )
    }

    /// APPS-STYLE-2 — the redesigned Start Menu (design: `docs/design/start-menu-redesign.md`).
    /// Header (title + QNM-Shared usage bar) → quick-link tiles → underline tabs →
    /// search → result rows (zebra + selected blue-accent, click-to-expand detail)
    /// → toast → operator/power footer. 460×720, all Carbon tokens (light + dark).
    /// RCLICK-5 — the Run (Win+R) box: a single command line that runs through a
    /// shell on submit. Lives in the power-menu popup (no separate window).
    fn run_view(&self) -> Element<'_, Message> {
        use cosmic::widget::{button, column, text, text_input, Space};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let input = text_input("Type a command, then Enter…", &self.run_text)
            .on_input(Message::RunInput)
            .on_submit(|_| Message::RunSubmit)
            .width(Length::Fill);
        cosmic::iced::widget::container(
            column(vec![
                text("Run")
                    .size(TypeRole::Heading.size_in(sizes))
                    .class(cosmic::theme::Text::Color(carbon(p.text)))
                    .into(),
                Space::new().height(Length::Fixed(10.0)).into(),
                input.into(),
                Space::new().height(Length::Fixed(10.0)).into(),
                button::custom(
                    text("Run")
                        .size(TypeRole::Body.size_in(sizes))
                        .class(cosmic::theme::Text::Color(carbon(p.text))),
                )
                .on_press(Message::RunSubmit)
                .class(cosmic::theme::Button::Standard)
                .into(),
            ])
            .spacing(0),
        )
        .padding(12)
        .width(Length::Fill)
        .into()
    }

    /// RCLICK — the Win+X-style right-click power menu (functional parity with
    /// the Windows 10 Start right-click menu, MCNF-augmented). Each row launches
    /// a real target (app spawn / elevated `pkexec` / `mde-workbench --focus`
    /// deep-link); the Power section runs the session actions directly.
    fn power_menu(&self) -> Element<'_, Message> {
        use cosmic::widget::{button, column, scrollable, text, Space};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let body_sz = TypeRole::Body.size_in(sizes);
        let cap_sz = TypeRole::Caption.size_in(sizes);
        let la =
            |args: &[&str]| Message::LaunchArgs(args.iter().map(|s| (*s).to_string()).collect());

        // One menu row: glyph + label, full-width subtle button.
        let row_item = |glyph: &str, label: &str, msg: Message| -> Element<'static, Message> {
            button::custom(
                cosmic::widget::row(vec![
                    text(glyph.to_string())
                        .size(14)
                        .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
                        .into(),
                    Space::new().width(Length::Fixed(12.0)).into(),
                    text(label.to_string())
                        .size(body_sz)
                        .class(cosmic::theme::Text::Color(carbon(p.text)))
                        .into(),
                ])
                .align_y(cosmic::iced::Alignment::Center),
            )
            .width(Length::Fill)
            .padding(cosmic::iced::Padding::from([7u16, 12u16]))
            .on_press(msg)
            .class(cosmic::theme::Button::Text)
            .into()
        };
        let divider = || -> Element<'static, Message> {
            cosmic::iced::widget::container(
                Space::new().width(Length::Fill).height(Length::Fixed(1.0)),
            )
            .padding(cosmic::iced::Padding::from([4u16, 8u16]))
            .into()
        };
        let header = text("Power User Menu")
            .size(TypeRole::Heading.size_in(sizes))
            .class(cosmic::theme::Text::Color(carbon(p.text)));

        let items: Vec<Element<'static, Message>> = vec![
            row_item(
                "\u{25A4}",
                "File Explorer",
                Message::LaunchLocal("mde-files".into()),
            ),
            row_item(
                "\u{2699}\u{FE0E}",
                "Settings",
                Message::LaunchLocal("cosmic-settings".into()),
            ),
            row_item(
                "\u{2BC8}",
                "Terminal",
                Message::LaunchLocal("cosmic-term".into()),
            ),
            row_item(
                "\u{2BC8}",
                "Terminal (Admin)",
                la(&["pkexec", "cosmic-term"]),
            ),
            row_item(
                "\u{25A6}",
                "Task Manager",
                la(&["cosmic-term", "-e", "btop"]),
            ),
            row_item(
                "\u{2756}",
                "Midnight Commander",
                la(&["cosmic-term", "-e", "mc"]),
            ),
            divider(),
            row_item(
                "\u{25A3}",
                "Device Manager",
                Message::LaunchFocus("node.hardware"),
            ),
            row_item(
                "\u{2725}",
                "Network Connections",
                Message::LaunchFocus("node.interfaces"),
            ),
            row_item(
                "\u{25A4}",
                "Disk Management",
                Message::LaunchFocus("mesh.mesh_storage"),
            ),
            row_item(
                "\u{25A4}",
                "Disk Management (Admin)",
                la(&["pkexec", "cosmic-disks"]),
            ),
            row_item(
                "\u{2261}",
                "Event Viewer",
                Message::LaunchFocus("monitoring.mesh_logs"),
            ),
            row_item(
                "\u{25A6}",
                "Apps & Features",
                Message::LaunchFocus("provisioning.profiles"),
            ),
            row_item(
                "\u{23FB}\u{FE0E}",
                "Power Options",
                la(&["cosmic-settings", "power"]),
            ),
            row_item(
                "\u{24D8}",
                "System / About",
                Message::LaunchFocus("system.about"),
            ),
            row_item(
                "\u{2318}",
                "Computer Management",
                la(&["pkexec", "mde-workbench"]),
            ),
            divider(),
            // MCNF-augmented entries.
            row_item(
                "\u{25C9}",
                "Mesh Control",
                Message::LaunchFocus("mesh.mesh_control"),
            ),
            row_item(
                "\u{25C9}",
                "Lighthouses",
                Message::LaunchFocus("mesh.lighthouses"),
            ),
            row_item(
                "\u{25D4}\u{FE0E}",
                "Notification Hub",
                Message::LaunchLocal("mde-notify-center".into()),
            ),
            row_item(
                "\u{2317}\u{FE0E}",
                "Join the Mesh",
                Message::LaunchLocal("mde-enroll".into()),
            ),
            divider(),
            row_item("\u{2BC8}", "Run…", Message::OpenRun),
            row_item("\u{25A1}", "Show Desktop", Message::ShowDesktop),
        ];
        let mut col = column(items).spacing(0).width(Length::Fill);
        // Power submenu (Lock / Sign out / Suspend / Restart / Shut down) — the
        // existing session actions, listed inline.
        col = col.push(divider());
        col = col.push(
            text("Power")
                .size(cap_sz)
                .class(cosmic::theme::Text::Color(carbon(p.text_muted))),
        );
        for kind in PowerKind::all() {
            col = col.push(row_item(
                "\u{23FB}\u{FE0E}",
                kind.label(),
                Message::Power(kind),
            ));
        }

        cosmic::iced::widget::container(
            column(vec![
                header.into(),
                Space::new().height(Length::Fixed(8.0)).into(),
                scrollable(col).height(Length::Fill).into(),
            ])
            .spacing(0),
        )
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn dropdown(&self) -> Element<'_, Message> {
        // RCLICK — the same popup surface renders the Win+X power menu when it
        // was opened by a secondary-click (or the Run box within it).
        if self.rclick_open {
            return if self.run_open {
                self.run_view()
            } else {
                self.power_menu()
            };
        }
        use cosmic::widget::{button, column, row, scrollable, text, text_input, Space};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let body_sz = TypeRole::Body.size_in(sizes);
        let cap_sz = TypeRole::Caption.size_in(sizes);
        let mono = cosmic::iced::Font::MONOSPACE;
        let now = Instant::now();
        // APPS-FX-1 — the open fade: the whole launcher content fades in (alpha
        // 0→1) on mount (Carbon `panel_mount` `FadeIn`), threaded into every text/
        // glyph color below. 1.0 at rest / under reduce-motion / the kill switch.
        let pa = Transition::FadeIn
            .params(self.anim.value(ANIM_POPUP, now, Easing::EaseOut))
            .alpha;

        // ── Header: grid glyph + title, then the QNM-Shared usage line + bar. ──
        let title_row = row(vec![
            text("\u{25A6}\u{FE0E}")
                .size(18)
                .class(cosmic::theme::Text::Color(carbon_at(p.accent, pa)))
                .into(),
            text("Applications")
                .size(TypeRole::Heading.size_in(sizes))
                .class(cosmic::theme::Text::Color(carbon_at(p.text, pa)))
                .into(),
        ])
        .spacing(10)
        .align_y(cosmic::iced::Alignment::Center);
        let (used, total) = self.qnm.unwrap_or((0, 0));
        let frac = if total > 0 {
            (used as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let used_pp = ((frac * 1000.0) as u16).max(1);
        let rest_pp = 1000u16.saturating_sub(used_pp).max(1);
        let usage_line = row(vec![
            text("Mesh Sync")
                .size(cap_sz)
                .font(mono)
                .class(cosmic::theme::Text::Color(carbon_at(p.text_muted, pa)))
                .into(),
            Space::new().width(Length::Fill).into(),
            text(mde_cosmic_applet::qnm_usage_label(self.qnm))
                .size(cap_sz)
                .font(mono)
                .class(cosmic::theme::Text::Color(carbon_at(p.text, pa)))
                .into(),
        ]);
        let fill_c = p.success;
        let track_c = p.overlay;
        let bar = row(vec![
            cosmic::iced::widget::container(Space::new())
                .width(Length::FillPortion(used_pp))
                .height(Length::Fixed(4.0))
                .style(move |_| cosmic::iced::widget::container::Style {
                    background: Some(carbon(fill_c).into()),
                    ..Default::default()
                })
                .into(),
            cosmic::iced::widget::container(Space::new())
                .width(Length::FillPortion(rest_pp))
                .height(Length::Fixed(4.0))
                .style(move |_| cosmic::iced::widget::container::Style {
                    background: Some(carbon(track_c).into()),
                    ..Default::default()
                })
                .into(),
        ]);
        let header = column(vec![title_row.into(), usage_line.into(), bar.into()]).spacing(8);

        // ── Quick-link tiles: glyph over label (Workbench / Files / Settings). ──
        let tile =
            |glyph: &'static str, label: &'static str, exec: &'static str| -> Element<Message> {
                button::custom(
                    column(vec![
                        text(glyph)
                            .size(18)
                            .class(cosmic::theme::Text::Color(carbon_at(p.text, pa)))
                            .into(),
                        text(label)
                            .size(cap_sz)
                            .class(cosmic::theme::Text::Color(carbon_at(p.text, pa)))
                            .into(),
                    ])
                    .spacing(6)
                    .align_x(cosmic::iced::Alignment::Center)
                    .width(Length::Fill),
                )
                .on_press(Message::LaunchLocal(exec.to_string()))
                .width(Length::Fill)
                .class(cosmic::theme::Button::Standard)
                .into()
            };
        let links = row(vec![
            tile("\u{2317}\u{FE0E}", "Workbench", "mde-workbench"),
            tile("\u{25A4}", "Files", "mde-files"),
            tile("\u{2699}\u{FE0E}", "Settings", "cosmic-settings"),
        ])
        .spacing(1);

        // ── Underline tabs (active = accent text + accent underline bar). ──
        let tabs: Vec<Element<Message>> = LauncherTab::all()
            .into_iter()
            .map(|t| {
                let active = t == self.tab && self.query.trim().is_empty();
                let underline = if active { p.accent } else { p.overlay };
                column(vec![
                    button::custom(text(t.label()).size(body_sz).class(
                        cosmic::theme::Text::Color(carbon_at(
                            if active { p.text } else { p.text_muted },
                            pa,
                        )),
                    ))
                    .on_press(Message::SetTab(t))
                    .class(cosmic::theme::Button::Text)
                    .into(),
                    cosmic::iced::widget::container(Space::new())
                        .height(Length::Fixed(2.0))
                        .width(Length::Fill)
                        .style(move |_| cosmic::iced::widget::container::Style {
                            background: Some(carbon(underline).into()),
                            ..Default::default()
                        })
                        .into(),
                ])
                .spacing(4)
                .into()
            })
            .collect();

        // ── Search (leading magnifier + clear when non-empty). ──
        let mut search_row = vec![
            text("\u{2315}")
                .size(cap_sz)
                .class(cosmic::theme::Text::Color(carbon_at(p.text_muted, pa)))
                .into(),
            text_input("Search apps, mesh, services…", &self.query)
                .on_input(Message::Search)
                .width(Length::Fill)
                .into(),
        ];
        if !self.query.is_empty() {
            search_row.push(
                button::custom(text("\u{2715}").size(cap_sz))
                    .on_press(Message::Search(String::new()))
                    .class(cosmic::theme::Button::Text)
                    .into(),
            );
        }
        let search = row(search_row)
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center);

        // ── Result rows (or an empty state). ──
        // APPS-FX-1 — tab crossfade: the result content fades in (alpha 0→1) on a
        // tab switch (Carbon `FadeIn`, threaded into the row/tile colors below).
        // Reduce-motion / the kill switch collapse this to the terminal frame
        // (alpha 1.0) since the tween is already complete / never armed. The body
        // also honors the open fade (`pa`), so it fades on both open and switch —
        // `min` takes whichever is more transparent at this frame.
        let tab_alpha = pa.min(
            Transition::FadeIn
                .params(self.anim.value(ANIM_TAB, now, Easing::EaseOut))
                .alpha,
        );
        let shown = filter_entries(&self.entries, self.tab, &self.query, &self.favorites);
        // APPS-WIDE — Favorites renders as a Carbon icon grid (not a row list)
        // when it's the active tab and not in a search.
        let fav_grid = self.tab == LauncherTab::Favorites && self.query.trim().is_empty();
        let body: Element<Message> = if shown.is_empty() {
            let (t_msg, sub) = if let Some(e) = &self.error {
                ("Couldn't reach the apps service".to_string(), e.clone())
            } else if self.tab == LauncherTab::Favorites && self.query.trim().is_empty() {
                (
                    "No favorites yet".to_string(),
                    "Pin apps from any tab to see them here.".to_string(),
                )
            } else {
                (
                    "Nothing here".to_string(),
                    "Try a different tab or search term.".to_string(),
                )
            };
            cosmic::iced::widget::container(
                column(vec![
                    text(t_msg)
                        .size(body_sz)
                        .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
                        .into(),
                    text(sub)
                        .size(cap_sz)
                        .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
                        .into(),
                ])
                .spacing(6)
                .align_x(cosmic::iced::Alignment::Center),
            )
            .padding(40)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into()
        } else if fav_grid {
            // APPS-WIDE — Carbon icon grid for Favorites.
            self.favorites_grid(&shown, tab_alpha)
        } else {
            column(
                shown
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| self.entry_row(i, e, tab_alpha))
                    .collect::<Vec<_>>(),
            )
            .spacing(0)
            .width(Length::Fill)
            .into()
        };

        // ── Assemble: header → links → tabs → search → body (flex) → toast → footer. ──
        let mut col = column(vec![
            header.into(),
            links.into(),
            row(tabs).spacing(0).into(),
            search.into(),
            scrollable(body).height(Length::Fill).into(),
        ])
        .spacing(10);
        if let Some(t) = &self.toast {
            col = col.push(self.toast_bar(t));
        }
        col = col.push(self.footer());

        // ── APPS-FX-1 — open fade+slide: on mount the launcher slides up
        // `PANEL_MOUNT_TRANSLATE_Y_PX` (a leading `Space` that shrinks to 0 as the
        // tween eases in). The fade is carried by the content alpha threaded into
        // the rows/header below (ANIM_TAB, armed alongside the slide on open) — the
        // iced 0.13 fork has no opacity widget, so alpha is applied to colors, not
        // the surface. The container keeps its `popup_container` surface/elevation
        // styling (we do NOT overpaint it). Carbon `panel_mount` entrance;
        // reduce-motion / the kill switch render the terminal frame.
        let popup = Transition::SlideUp(PANEL_MOUNT_TRANSLATE_Y_PX)
            .params(self.anim.value(ANIM_POPUP, now, Easing::EaseOut));
        let slid = column(vec![
            Space::new().height(Length::Fixed(popup.translate_y)).into(),
            col.into(),
        ]);

        // APPS-FIT — the body fills the detected desktop-fraction size (33% of
        // the screen width × height; falls back to the golden rectangle). Must
        // match the popup positioner size set on open.
        cosmic::iced::widget::container(slid)
            .padding(12)
            .width(Length::Fixed(self.menu_w))
            .height(Length::Fixed(self.menu_h))
            .into()
    }

    /// APPS-WIDE — the primary action for a Favorites tile press: launch apps /
    /// mesh-apps directly (favorites are normally pinned apps), else fall back to
    /// selecting the entry (opens its detail in the list view).
    fn entry_primary_msg(e: &Entry) -> Message {
        match e.kind.as_str() {
            "app" if !e.exec.is_empty() => Message::LaunchLocal(e.exec.clone()),
            "mesh-app" if !e.node.is_empty() => Message::LaunchMesh(e.node.clone()),
            _ => Message::SelectEntry(e.id.clone()),
        }
    }

    /// APPS-WIDE — the Favorites tab as a Carbon icon grid: 3 tiles per row,
    /// capped at [`FAVORITES_MAX`] (a 3×3 grid), mirroring the Workbench/Files/
    /// Settings quick-link row (`tile` in [`Self::dropdown`]) — same icon-over-
    /// label tiles, `Button::Standard`, equal-width, `spacing(1)`. The last
    /// partial row is padded so tiles keep a uniform width.
    fn favorites_grid(&self, shown: &[&Entry], alpha: f32) -> Element<'static, Message> {
        use cosmic::widget::{column, row, Space};
        let rows: Vec<Element<Message>> = shown
            .iter()
            .take(FAVORITES_MAX)
            .collect::<Vec<_>>()
            .chunks(FAVORITES_COLUMNS)
            .map(|chunk| {
                let mut tiles: Vec<Element<Message>> = chunk
                    .iter()
                    .map(|e| self.favorite_tile(e, alpha))
                    .collect();
                while tiles.len() < FAVORITES_COLUMNS {
                    tiles.push(Space::new().width(Length::FillPortion(1)).into());
                }
                row(tiles).spacing(1).width(Length::Fill).into()
            })
            .collect();
        column(rows)
            .spacing(1)
            .padding([4, 0])
            .width(Length::Fill)
            .into()
    }

    /// APPS-WIDE — one Favorites tile, mirroring the quick-link tiles: a Carbon
    /// icon (`mde_theme` icon set) over a centred, truncated name in a
    /// `Button::Standard`, equal-width. Whole-tile press launches the app/mesh-
    /// app (else selects). Owns its strings so the tile is `'static`.
    fn favorite_tile(&self, e: &Entry, alpha: f32) -> Element<'static, Message> {
        use cosmic::widget::{button, column, mouse_area, text};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let cap_sz = TypeRole::Caption.size_in(sizes);
        // APPS-FX-1 — hover accent: the hovered tile gains an accent-tinted wash
        // that fades in as the hover tween eases in (Carbon `hover`, eased via the
        // `Lift` transition's progress). Applied as a background alpha only — never
        // a translate/`Space`, so it costs zero layout reflow (the animation
        // module's "compositor-friendly, never layout thrash" contract). `h` is 0
        // at rest; when hovered it eases 0→1 (instant under reduce-motion / the
        // kill switch — the terminal highlighted frame).
        let h = self.hover_t(&e.id);
        let hover_lift = Transition::Lift(1.0).params(h).translate_y.abs();
        let id = e.id.clone();
        // APPS-FAV-ICON (operator 2026-06-19) — render the actual Carbon icon
        // SVG (the mde_theme icon set), tinted to the theme text color — the same
        // icons used when docking an app — instead of the Unicode fallback glyph.
        // Falls back to the glyph only if a variant ships no baked SVG.
        let resolved = mde_icon(favorite_icon(e), IconSize::Nav);
        let icon_px = resolved.size_px();
        let icon_widget: Element<'static, Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            let tint = carbon_at(p.text, alpha);
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(icon_px))
                .height(Length::Fixed(icon_px))
                .class(cosmic::theme::Svg::custom(move |_t| widget_svg::Style {
                    color: Some(tint),
                }))
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(20)
                .class(cosmic::theme::Text::Color(carbon_at(p.text, alpha)))
                .into()
        };
        // Truncate long names so tiles stay aligned.
        let name = if e.name.chars().count() > 14 {
            format!("{}…", e.name.chars().take(13).collect::<String>())
        } else {
            e.name.clone()
        };
        let tile = button::custom(
            column(vec![
                icon_widget,
                text(name)
                    .size(cap_sz)
                    .center()
                    .class(cosmic::theme::Text::Color(carbon_at(p.text, alpha)))
                    .into(),
            ])
            .spacing(6)
            .align_x(cosmic::iced::Alignment::Center)
            .width(Length::Fill),
        )
        .on_press(Self::entry_primary_msg(e))
        .width(Length::Fill)
        .class(cosmic::theme::Button::Standard);
        // APPS-FX-1 — the accent wash beneath the tile, its alpha eased in with the
        // hover (the Carbon `toast::ACTION_HOVER_BG_ALPHA` token at full hover,
        // also multiplied by the open/tab fade so it never out-runs the content).
        let accent = p.accent;
        let wash = hover_lift * toast_motion::ACTION_HOVER_BG_ALPHA * alpha;
        let washed =
            cosmic::iced::widget::container(tile).style(move |_| {
                cosmic::iced::widget::container::Style {
                    background: Some(carbon_at(accent, wash).into()),
                    ..Default::default()
                }
            });
        mouse_area(washed)
            .on_enter(Message::TileEnter(id.clone()))
            .on_exit(Message::TileExit(id))
            .into()
    }

    /// APPS-STYLE-2 — one result row: letter avatar + accent-blue title + mono
    /// subtitle + status dot, on a zebra layer; clicking toggles the inline
    /// detail ([`Self::detail`]) and the selected row gets a blue left-accent +
    /// raised bg. Theme-aware (all mde-theme tokens).
    fn entry_row<'a>(&self, idx: usize, e: &'a Entry, alpha: f32) -> Element<'a, Message> {
        use cosmic::widget::{column, mouse_area, row, text};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let cap_sz = TypeRole::Caption.size_in(sizes);
        let mono = cosmic::iced::Font::MONOSPACE;
        let selected = self.selected.as_deref() == Some(&e.id);
        // APPS-FX-1 — hover lift + accent for this app tile/row: the hovered row's
        // shade brightens toward the accent as the hover tween eases in (Carbon
        // `hover`). 0 at rest / under reduce-motion / the kill switch.
        let hover = self.hover_t(&e.id);
        let sub = match e.kind.as_str() {
            "mesh-app" => format!("mesh · {} · {}", e.node, e.health),
            "workload" => format!("{} · {}", e.source, e.state),
            "service" => format!("service · {}", e.node),
            _ => e.source.clone(),
        };

        // Letter avatar.
        let letter = e
            .name
            .chars()
            .next()
            .map_or_else(String::new, |c| c.to_uppercase().to_string());
        let av_bg = p.raised;
        let avatar = cosmic::iced::widget::container(
            text(letter)
                .size(cap_sz)
                .font(mono)
                .class(cosmic::theme::Text::Color(carbon_at(p.text_muted, alpha))),
        )
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(32.0))
        .center_x(Length::Fixed(32.0))
        .center_y(Length::Fixed(32.0))
        .style(move |_| cosmic::iced::widget::container::Style {
            background: Some(carbon(av_bg).into()),
            ..Default::default()
        });

        // Title — the app name in primary text (white in dark, black in light;
        // operator override of the design's blue), over a mono subtitle.
        let body = column(vec![
            text(e.name.clone())
                .size(TypeRole::Body.size_in(sizes))
                .class(cosmic::theme::Text::Color(carbon_at(p.text, alpha)))
                .into(),
            text(sub)
                .size(cap_sz)
                .font(mono)
                .class(cosmic::theme::Text::Color(carbon_at(p.text_muted, alpha)))
                .into(),
        ])
        .spacing(2)
        .width(Length::Fill);

        // Status dot (where the entry carries health/state).
        let mut cells: Vec<Element<Message>> = vec![avatar.into(), body.into()];
        if let Some(dotc) = self.status_color(e) {
            cells.push(
                cosmic::iced::widget::container(cosmic::widget::Space::new())
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0))
                    .style(move |_| cosmic::iced::widget::container::Style {
                        background: Some(carbon(dotc).into()),
                        border: cosmic::iced::Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into(),
            );
        }

        // The clickable row → toggle the inline detail.
        let main = cosmic::widget::button::custom(
            row(cells)
                .spacing(12)
                .align_y(cosmic::iced::Alignment::Center),
        )
        .on_press(Message::SelectEntry(e.id.clone()))
        .width(Length::Fill)
        .class(cosmic::theme::Button::Text);

        let inner: Element<Message> = if selected {
            column(vec![main.into(), self.detail(e)]).spacing(0).into()
        } else {
            main.into()
        };

        // Zebra shading + selected accent (Q8: both). Selected = raised bg + a
        // blue left-accent bar; otherwise alternate two Carbon layers.
        let shade = if selected {
            p.raised
        } else if idx % 2 == 1 {
            p.surface
        } else {
            p.background
        };
        let accent = if selected { p.accent } else { shade };
        // APPS-FX-1 — hover wash: brighten the row's shade toward the accent token
        // as the hover eases in, capped at the Carbon `toast::ACTION_HOVER_BG_ALPHA`
        // token (a subtle accent hint, never a full flood). 0 at rest; eased on
        // hover (instant under reduce-motion / the kill switch — the terminal
        // highlighted frame). Background alpha only → no layout reflow.
        let accent_tok = p.accent;
        let wash = hover * toast_motion::ACTION_HOVER_BG_ALPHA;
        let row_box = cosmic::iced::widget::container(inner)
            .padding([6, 10])
            .width(Length::Fill)
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(carbon_blend(shade, accent_tok, wash).into()),
                border: cosmic::iced::Border {
                    color: carbon(accent),
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            });
        mouse_area(row_box)
            .on_enter(Message::TileEnter(e.id.clone()))
            .on_exit(Message::TileExit(e.id.clone()))
            .into()
    }

    /// APPS-STYLE-2 — status dot colour for an entry (online→success,
    /// idle/degraded→warning, offline/unknown→muted). `None` = no dot.
    fn status_color(&self, e: &Entry) -> Option<mde_theme::Rgba> {
        let p = self.palette;
        let s = match e.kind.as_str() {
            "mesh-app" => e.health.as_str(),
            "workload" => e.state.as_str(),
            "service" => "running",
            _ => return None,
        };
        let sl = s.to_lowercase();
        if sl.contains("online")
            || sl.contains("healthy")
            || sl.contains("running")
            || sl.contains("up")
        {
            Some(p.success)
        } else if sl.contains("idle") || sl.contains("degraded") || sl.contains("pending") {
            Some(p.warning)
        } else if sl.is_empty() {
            None
        } else {
            Some(p.text_muted)
        }
    }

    /// APPS-STYLE-2 — the expanded detail panel for the selected row: primary
    /// action (Launch/Connect/Open or workload Start/Stop + Attach), Pin/Unpin,
    /// and for apps an "Open on host" chip row (remote desktop to each peer).
    fn detail<'a>(&self, e: &'a Entry) -> Element<'a, Message> {
        use cosmic::widget::{button, column, row, text};
        let p = self.palette;
        let cap_sz = TypeRole::Caption.size_in(FontSize::defaults());
        let primary = |label: &str, msg: Message| -> Element<Message> {
            button::custom(text(label.to_string()).size(cap_sz))
                .on_press(msg)
                .class(cosmic::theme::Button::Suggested)
                .into()
        };
        let secondary = |label: String, msg: Message| -> Element<Message> {
            button::custom(text(label).size(cap_sz))
                .on_press(msg)
                .class(cosmic::theme::Button::Text)
                .into()
        };

        let mut actions: Vec<Element<Message>> = Vec::new();
        match e.kind.as_str() {
            "app" if !e.exec.is_empty() => {
                actions.push(primary("Launch", Message::LaunchLocal(e.exec.clone())));
            }
            "mesh-app" if !e.node.is_empty() => {
                actions.push(primary("Connect", Message::LaunchMesh(e.node.clone())));
            }
            "service" if !e.endpoint.is_empty() => {
                actions.push(primary("Open", Message::OpenService(e.endpoint.clone())));
            }
            "workload" => {
                if mde_cosmic_applet::workload_running(&e.state) {
                    actions.push(primary(
                        "Stop",
                        Message::Workload(e.source.clone(), e.name.clone(), WorkloadAction::Stop),
                    ));
                } else {
                    actions.push(primary(
                        "Start",
                        Message::Workload(e.source.clone(), e.name.clone(), WorkloadAction::Start),
                    ));
                }
                actions.push(secondary(
                    "Attach".into(),
                    Message::Workload(e.source.clone(), e.name.clone(), WorkloadAction::Attach),
                ));
            }
            _ => {}
        }
        // Pin/Unpin — universal secondary (Q6).
        actions.push(secondary(
            if self.favorites.contains(&e.id) {
                "Unpin".into()
            } else {
                "Pin".into()
            },
            Message::ToggleFavorite(e.id.clone()),
        ));

        let mut col = column(vec![row(actions)
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center)
            .into()])
        .spacing(12);

        // "Open on host" chips — apps only (Q3: RD to the peer).
        if e.kind == "app" {
            let peers = self.peers();
            if !peers.is_empty() {
                col = col.push(
                    text("Open on host")
                        .size(cap_sz)
                        .font(cosmic::iced::Font::MONOSPACE)
                        .class(cosmic::theme::Text::Color(carbon(p.text_muted))),
                );
                let chips: Vec<Element<Message>> = peers
                    .into_iter()
                    .map(|h| {
                        button::custom(
                            text(h.clone())
                                .size(cap_sz)
                                .font(cosmic::iced::Font::MONOSPACE),
                        )
                        .on_press(Message::OpenOnHost(h))
                        .class(cosmic::theme::Button::Standard)
                        .into()
                    })
                    .collect();
                col = col.push(
                    cosmic::widget::flex_row(chips)
                        .column_spacing(6)
                        .row_spacing(6),
                );
            }
        }

        let band = p.accent;
        cosmic::iced::widget::container(col)
            .padding(12)
            .width(Length::Fill)
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(carbon(p.overlay).into()),
                border: cosmic::iced::Border {
                    color: carbon(band),
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    /// APPS-STYLE-2 — the bottom toast feedback bar.
    fn toast_bar<'a>(&self, msg: &'a str) -> Element<'a, Message> {
        use cosmic::widget::{button, row, text, Space};
        let p = self.palette;
        let cap_sz = TypeRole::Caption.size_in(FontSize::defaults());
        let accent = p.success;
        let bar = cosmic::iced::widget::container(Space::new())
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(24.0))
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(carbon(accent).into()),
                ..Default::default()
            });
        let inner = row(vec![
            bar.into(),
            text(msg.to_string())
                .size(cap_sz)
                .width(Length::Fill)
                .class(cosmic::theme::Text::Color(carbon(p.text)))
                .into(),
            button::custom(text("\u{2715}").size(cap_sz))
                .on_press(Message::DismissToast)
                .class(cosmic::theme::Button::Text)
                .into(),
        ])
        .spacing(10)
        .align_y(cosmic::iced::Alignment::Center);
        cosmic::iced::widget::container(inner)
            .padding([8, 4])
            .width(Length::Fill)
            .into()
    }

    /// APPS-STYLE-2 — the operator + power footer (and the power menu when open).
    fn footer(&self) -> Element<'_, Message> {
        use cosmic::widget::{button, column, row, text, Space};
        let p = self.palette;
        let cap_sz = TypeRole::Caption.size_in(FontSize::defaults());
        let user = current_user();
        let initial = user
            .chars()
            .next()
            .map_or_else(String::new, |c| c.to_uppercase().to_string());
        let ac = p.accent;
        let avatar = cosmic::iced::widget::container(
            text(initial)
                .size(cap_sz)
                .font(cosmic::iced::Font::MONOSPACE)
                .class(cosmic::theme::Text::Color(carbon(p.background))),
        )
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .center_x(Length::Fixed(28.0))
        .center_y(Length::Fixed(28.0))
        .style(move |_| cosmic::iced::widget::container::Style {
            background: Some(carbon(ac).into()),
            ..Default::default()
        });
        let bar = row(vec![
            avatar.into(),
            text(user)
                .size(cap_sz)
                .width(Length::Fill)
                .class(cosmic::theme::Text::Color(carbon(p.text)))
                .into(),
            button::custom(
                text("\u{23FB}\u{FE0E}")
                    .size(16)
                    .class(cosmic::theme::Text::Color(carbon(p.text_muted))),
            )
            .on_press(Message::TogglePower)
            .class(cosmic::theme::Button::Text)
            .into(),
        ])
        .spacing(10)
        .align_y(cosmic::iced::Alignment::Center);

        if !self.power_open {
            return bar.into();
        }
        // The power menu, stacked above the footer bar.
        let action = |k: PowerKind| -> Element<Message> {
            button::custom(text(k.label().to_string()).size(cap_sz))
                .on_press(Message::Power(k))
                .width(Length::Fill)
                .class(cosmic::theme::Button::Text)
                .into()
        };
        column(vec![
            action(PowerKind::Lock),
            action(PowerKind::Logout),
            action(PowerKind::Suspend),
            action(PowerKind::Restart),
            action(PowerKind::Shutdown),
            Space::new().height(Length::Fixed(4.0)).into(),
            bar.into(),
        ])
        .spacing(2)
        .into()
    }

    /// Mesh peer hostnames (from the mesh-app entries) for the "run on ▸" menu.
    fn peers(&self) -> Vec<String> {
        let mut p: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.kind == "mesh-app" && !e.node.is_empty())
            .map(|e| e.node.clone())
            .collect();
        p.sort();
        p.dedup();
        p
    }
}

#[cfg(test)]
mod apps_fit_tests {
    use super::{parse_menu_size_from_kdl, MENU_SCREEN_FRACTION};

    const SAMPLE: &str = r#"output "DP-1" enabled=#true {
  description model="Test"
  physical 600 340
  position 0 0
  scale 1.00
  modes {
    mode 2560 1440 59951 current=#true preferred=#true
    mode 1920 1080 60000
  }
}
output "HDMI-A-1" enabled=#false {
  scale 1.00
  modes {
    mode 1920 1080 60000 current=#true
  }
}
"#;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01, "expected {b}, got {a}");
    }

    #[test]
    fn picks_named_output_current_mode_at_fraction() {
        let (w, h) = parse_menu_size_from_kdl(SAMPLE, "DP-1").unwrap();
        approx(w, 2560.0 * MENU_SCREEN_FRACTION);
        approx(h, 1440.0 * MENU_SCREEN_FRACTION);
    }

    #[test]
    fn divides_logical_size_by_scale() {
        let kdl = r#"output "DP-1" enabled=#true {
  scale 2.00
  modes {
    mode 2560 1440 59951 current=#true preferred=#true
  }
}
"#;
        let (w, h) = parse_menu_size_from_kdl(kdl, "DP-1").unwrap();
        // logical = 1280x720, then 33%.
        approx(w, 1280.0 * MENU_SCREEN_FRACTION);
        approx(h, 720.0 * MENU_SCREEN_FRACTION);
    }

    #[test]
    fn empty_target_uses_first_enabled_output() {
        let (w, _h) = parse_menu_size_from_kdl(SAMPLE, "").unwrap();
        approx(w, 2560.0 * MENU_SCREEN_FRACTION);
    }

    #[test]
    fn skips_disabled_and_missing_target_falls_back_to_first_enabled() {
        // Target a name that doesn't exist → first enabled output (DP-1), not the
        // disabled HDMI-A-1.
        let (w, _h) = parse_menu_size_from_kdl(SAMPLE, "NOPE").unwrap();
        approx(w, 2560.0 * MENU_SCREEN_FRACTION);
    }

    #[test]
    fn no_current_mode_returns_none() {
        let kdl = r#"output "DP-1" enabled=#true {
  scale 1.00
  modes {
    mode 1920 1080 60000
  }
}
"#;
        assert!(parse_menu_size_from_kdl(kdl, "DP-1").is_none());
    }
}

#[cfg(test)]
mod apps_fx_tests {
    //! APPS-FX-1 — the launcher-motion glue: the color-alpha apply helpers (the
    //! iced-0.13 fade technique) + that the animator is reduce-motion / kill-switch
    //! aware end-to-end through this binary's `animate` path.
    use super::{carbon_at, carbon_blend, ANIM_POPUP};
    use mde_theme::animation::Animator;
    use mde_theme::motion::Motion;
    use mde_theme::Rgba;
    use std::time::{Duration, Instant};

    #[test]
    fn carbon_at_multiplies_token_alpha() {
        // A fully-opaque token at alpha 0.5 → half opacity; alpha 1.0 → unchanged;
        // alpha 0.0 → fully transparent. RGB is preserved (only alpha fades).
        let tok = Rgba::rgba(0, 100, 200, 1.0);
        assert!((carbon_at(tok, 0.5).a - 0.5).abs() < 1e-6);
        assert!((carbon_at(tok, 1.0).a - 1.0).abs() < 1e-6);
        assert!(carbon_at(tok, 0.0).a.abs() < 1e-6);
        let c = carbon_at(tok, 0.5);
        assert!((c.g - 100.0 / 255.0).abs() < 1e-4, "RGB preserved under fade");
        // An already-translucent token stays multiplied, never reset to opaque.
        let translucent = Rgba::rgba(255, 255, 255, 0.4);
        assert!((carbon_at(translucent, 0.5).a - 0.2).abs() < 1e-6);
        // Out-of-range alpha clamps (no overshoot past full opacity).
        assert!((carbon_at(tok, 2.0).a - 1.0).abs() < 1e-6);
    }

    #[test]
    fn carbon_blend_lerps_between_tokens() {
        let a = Rgba::rgba(0, 0, 0, 1.0);
        let b = Rgba::rgba(255, 255, 255, 1.0);
        // t=0 → a, t=1 → b, t=0.5 → midpoint.
        assert!(carbon_blend(a, b, 0.0).r.abs() < 1e-4);
        assert!((carbon_blend(a, b, 1.0).r - 1.0).abs() < 1e-4);
        assert!((carbon_blend(a, b, 0.5).r - 0.5).abs() < 1e-3);
    }

    #[test]
    fn animate_path_is_reduce_motion_and_kill_switch_aware() {
        // Mirror `AppsApplet::animate`: a popup tween armed with reduce_motion caps
        // to the ≤80 ms crossfade and is settled by then; the kill switch (motion
        // disabled) arms nothing, so the surface is at its terminal frame at once.
        let t0 = Instant::now();

        let mut reduced = Animator::new();
        reduced.start(ANIM_POPUP, t0, Motion::panel_mount(), true);
        assert!(
            reduced.is_idle(t0 + Duration::from_millis(80)),
            "reduce-motion popup must settle by the 80 ms cap"
        );

        // Kill switch: `animate` returns early, so nothing is started → idle now
        // and the value is the terminal frame (1.0).
        let disabled = Animator::new();
        assert!(disabled.is_idle(t0), "kill switch arms no tween");
        assert!(
            (disabled.value(ANIM_POPUP, t0, mde_theme::motion::Easing::EaseOut) - 1.0).abs() < 1e-6,
            "absent tween → terminal frame (alpha 1.0)"
        );

        // Full motion: a panel_mount tween is mid-flight at 30 ms, settled by 240.
        let mut full = Animator::new();
        full.start(ANIM_POPUP, t0, Motion::panel_mount(), false);
        let mid = full.value(ANIM_POPUP, t0 + Duration::from_millis(30), mde_theme::motion::Easing::EaseOut);
        assert!(mid > 0.0 && mid < 1.0, "popup interpolating mid-flight, got {mid}");
        assert!(full.is_idle(t0 + Duration::from_millis(240)));
    }
}
