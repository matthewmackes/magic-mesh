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
use cosmic::iced::{Length, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::{Application, Element};

use mde_bus::hooks::config::Priority;
use mde_cosmic_applet::{
    app_running, app_running_local, filter_entries, parse_entries, workload_argv, Entry,
    LauncherTab, WorkloadAction,
};
use mde_theme::animation::{Animator, Transition};
use mde_theme::{
    mde_icon, Easing, FontSize, Icon, IconSize, Motion, Palette, Preferences, Rgba, TypeRole,
};

const ID: &str = "com.mackes.MagicMeshApps";

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

/// APPS-FX-1 — animation tick period while motion is in flight (~60 fps). The
/// applet only ticks at this rate while the [`Animator`] has a live tween;
/// otherwise it falls back to the idle toggle poll (no busy loop — §7).
const ANIM_TICK_MS: u64 = 16;

/// APPS-FX-1 — the [`Animator`] key for the dropdown's open fade/scale-in.
const ANIM_MENU: &str = "menu";
/// APPS-FX-1 — the [`Animator`] key for the tab-switch crossfade (the result body
/// fades back in when the active tab changes).
const ANIM_TAB: &str = "tab";
/// APPS-FX-1 — prefix for a per-tile hover tween key (`hover:<surface>:<entry id>`).
const ANIM_HOVER_PREFIX: &str = "hover:";

/// APPS-FX-1 — the hover-lift rise (px). A tasteful Carbon micro-interaction:
/// the tile rises a hair on hover. Component dimension (not density-scaled), in
/// the same spirit as the shared `PANEL_MOUNT_TRANSLATE_Y_PX` token.
const TILE_HOVER_RISE_PX: f32 = 3.0;

/// APPS-FX-1 — the open animation's slide-up distance (px): the dropdown body
/// rises this far as it fades in. Mirrors the shared panel-mount translate token.
const MENU_SLIDE_PX: f32 = mde_theme::PANEL_MOUNT_TRANSLATE_Y_PX;

/// APPS-FX-1 — the [`Animator`] key for one hovered element, namespaced by its
/// `surface` (`"tile"` for the Favorites grid, `"row"` for the result list) so
/// the same entry id appearing as both a favorite tile and a list row drives two
/// independent tweens (no cross-talk).
fn hover_key(surface: &str, id: &str) -> String {
    format!("{ANIM_HOVER_PREFIX}{surface}:{id}")
}

/// APPS-FX-1 — hover surface discriminators (see [`hover_key`]).
const HOVER_TILE: &str = "tile";
const HOVER_ROW: &str = "row";

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
    /// APPS-9b — the last `event/apps/toggle` ULID acted on. Primed at init to
    /// the current bus head so a stale pre-launch signal never auto-opens; a
    /// newer ULID seen by the poll subscription flips the dropdown (so a baked
    /// Super shortcut running `--toggle` opens/closes the launcher).
    last_toggle_ulid: Option<String>,
    /// APPS-FX-1 — the shared animation registry (one clock drives every in-flight
    /// tween: menu open-in, tab crossfade, per-tile hover lift). Tick-driven only
    /// while non-idle, so there's no idle CPU cost (§7 / MOTION-PERF-1).
    anim: Animator,
    /// APPS-FX-1 — the [`hover_key`] of the element currently hovered (its tween
    /// rises 0→1), or `None`. Set from `mouse_area` enter/exit events. The key is
    /// surface-namespaced so a favorite tile and a list row never collide.
    hovered: Option<String>,
    /// APPS-FX-1 — the [`hover_key`] of the element that just lost hover, so it can
    /// ease *down* (1→0) over the same tween instead of snapping. Cleared when the
    /// tween settles. Only the most-recently-exited element eases down (you hover
    /// one at a time), others are already at rest.
    releasing: Option<String>,
    /// APPS-FX-1 — `a11y.reduce_motion` resolved from the user's preferences
    /// (refreshed with the palette on open); tweens collapse to the ≤80 ms Carbon
    /// crossfade when set (the reduce-motion contract).
    reduce_motion: bool,
    /// APPS-FX-1 — the global motion controls (kill switch + speed scale,
    /// MOTION-CORE-3). Every tween's duration is resolved through
    /// [`mde_theme::prefs::MotionPrefs::apply`] so a disabled/scaled preference is
    /// honored; refreshed with the palette on open.
    motion: mde_theme::prefs::MotionPrefs,
    /// APPS-LIVE-2 — this node's hostname (cached at init), so a click on a
    /// running app entry can tell "running here" (raise the window) from "running
    /// on a peer" (relaunch / remote-desktop).
    this_host: String,
}

#[derive(Clone, Debug)]
enum Message {
    /// Cosmic surface (popup create/destroy) action passthrough.
    Surface(cosmic::surface::Action),
    /// The popup was closed by the compositor.
    PopupClosed(Id),
    /// Open-or-close the launcher dropdown.
    TogglePopup,
    /// APPS-9b — poll tick: check the bus for a new `event/apps/toggle` signal
    /// (published by a baked Super shortcut) and flip the dropdown if so.
    PollToggleSignal,
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
    /// APPS-LIVE-2 — a running-on-this-node app was clicked: try to raise its
    /// existing window instead of relaunching, falling back to `exec` when focus
    /// isn't possible. Carries `(focus_hint, exec)` — the hint is the app's launch
    /// binary basename (its likely window class).
    FocusOrLaunchLocal(String, String),
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
    /// APPS-FX-1 — one animation frame: advance/GC the [`Animator`]. Emitted by
    /// the tick subscription only while a tween is in flight.
    AnimTick,
    /// APPS-FX-1 — an element gained pointer hover (start its lift/highlight),
    /// carrying its surface-namespaced [`hover_key`].
    HoverEnter(String),
    /// APPS-FX-1 — an element lost pointer hover (settle its lift/highlight back),
    /// carrying its [`hover_key`].
    HoverExit(String),
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

/// APPS-LIVE-2 — this node's hostname (matches the `hostname` mackesd's
/// `apps_running` worker stamps into `running-apps.json`), used to decide whether
/// a running app is live *here* (raise it) or on a peer (relaunch / remote). Reads
/// `/etc/hostname` (trimmed); falls back to the `HOSTNAME` env then `localhost`.
fn local_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "localhost".to_string())
}

/// APPS-9b — the bus topic a baked Super shortcut publishes (via `--toggle`) to
/// open/close the launcher. It's an `event/*` fire-and-forget signal, so it's
/// written to the bus directly rather than via the `action/`-gated
/// [`mde_bus::rpc::publish_request`] (which rejects non-`action/` topics).
const TOGGLE_TOPIC: &str = "event/apps/toggle";

/// APPS-9b — how often the panel applet polls the bus for a new toggle signal.
/// One indexed `latest_ulid` SQLite read; 120 ms keeps Super feeling instant
/// without measurable idle cost.
const TOGGLE_POLL_MS: u64 = 120;

/// Pure decision: should a freshly-read toggle ULID fire a toggle? Yes iff the
/// bus carries a toggle event whose ULID differs from the last one acted on. The
/// baseline is primed at startup so a stale pre-launch event never auto-opens.
fn should_toggle(last_seen: Option<&str>, current: Option<&str>) -> bool {
    matches!(current, Some(c) if last_seen != Some(c))
}

/// Append one `event/apps/toggle` signal to a bus store. Split from
/// [`publish_toggle`] so the publish→read round-trip is unit-testable against a
/// throwaway store.
fn write_toggle(persist: &mde_bus::persist::Persist) -> Result<String, String> {
    persist
        .write(TOGGLE_TOPIC, Priority::Min, None, None)
        .map(|m| m.ulid)
        .map_err(|e| format!("bus write: {e}"))
}

/// `mde-apps-applet --toggle` entrypoint: publish one toggle signal to the shared
/// bus and return. The long-running panel applet polls the topic and flips its
/// dropdown — so a baked Super shortcut need only run this.
fn publish_toggle() -> Result<String, String> {
    let dir = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
    let persist = mde_bus::persist::Persist::open(dir).map_err(|e| format!("bus store: {e}"))?;
    write_toggle(&persist)
}

/// The latest `event/apps/toggle` ULID on the shared bus, if any (the applet's
/// poll baseline). A missing bus / read error reads as "no signal".
fn latest_toggle_ulid() -> Option<String> {
    let dir = mde_bus::default_data_dir()?;
    let persist = mde_bus::persist::Persist::open(dir).ok()?;
    persist.latest_ulid(TOGGLE_TOPIC).ok().flatten()
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

/// APPS-LIVE-2 — the launch-binary basename of a `.desktop` exec line (the app's
/// likely window class / `app_id`), used as the focus hint when raising a running
/// instance. Strips XDG field codes + a path; `None` for an empty exec. Mirrors
/// the mackesd-side `apps_running::exec_basename` so the hint a click sends back
/// is the same token mackesd matched against `/proc`.
fn exec_focus_hint(exec: &str) -> Option<String> {
    for tok in exec.split_whitespace() {
        if tok.starts_with('%') || tok == "env" || tok.contains('=') {
            continue;
        }
        let base = std::path::Path::new(tok)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(tok);
        if !base.is_empty() {
            return Some(base.to_string());
        }
    }
    None
}

/// APPS-LIVE-2 — raise an already-running app's window, returning `true` when a
/// focus tool actually activated a matching window.
///
/// Native Wayland has no portable raise, but apps running under XWayland (the
/// common case for Firefox/GIMP/etc.) are reachable via `wmctrl`: `wmctrl -x -a
/// <class>` activates the first window whose WM class contains the token
/// (case-insensitive) — the `.desktop` launch binary is the usual class token.
///
/// Returns `false` when no tool is present or no window matched, so the caller
/// falls back to relaunch (the acceptance's "fall back to relaunch" path).
fn focus_local_window(hint: &str) -> bool {
    if hint.is_empty() {
        return false;
    }
    // `wmctrl -x -a <class>` raises the first window whose WM class contains the
    // token (case-insensitive). Exit status 0 = a window was activated.
    std::process::Command::new("wmctrl")
        .args(["-x", "-a", hint])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// APPS-LIVE-2 — try to focus a running local app by `hint`; relaunch via `exec`
/// when focus isn't possible (no tool / no matching window). Detached spawn for
/// the fallback (the NOTIFY-UI-4 no-zombie rule), same as [`launch_local`].
fn focus_or_launch_local(hint: &str, exec: &str) {
    if focus_local_window(hint) {
        return;
    }
    launch_local(exec);
}

fn carbon(c: Rgba) -> cosmic::iced::Color {
    cosmic::iced::Color {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: c.a,
    }
}

/// APPS-FX-1 — blend two Carbon token colors by `t` (0 = `a`, 1 = `b`), clamped.
/// Used to ease a row's background toward the raised highlight on hover (a
/// subtle, jank-free accent — no layout shift). Channel-wise `lerp_f32`.
fn carbon_mix(a: Rgba, b: Rgba, t: f32) -> cosmic::iced::Color {
    let (ca, cb) = (carbon(a), carbon(b));
    cosmic::iced::Color {
        r: mde_theme::lerp_f32(ca.r, cb.r, t),
        g: mde_theme::lerp_f32(ca.g, cb.g, t),
        b: mde_theme::lerp_f32(ca.b, cb.b, t),
        a: mde_theme::lerp_f32(ca.a, cb.a, t),
    }
}

fn main() -> cosmic::iced::Result {
    // APPS-9b — `mde-apps-applet --toggle` publishes one toggle signal to the bus
    // and exits (what a baked Super shortcut runs). The long-running panel applet
    // (no args) polls the topic and flips its dropdown.
    if std::env::args().skip(1).any(|a| a == "--toggle") {
        if let Err(e) = publish_toggle() {
            eprintln!("mde-apps-applet --toggle: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
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
        let prefs = Preferences::load();
        let reduce_motion = prefs.a11y.reduce_motion;
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
                palette: Palette::for_theme(prefs.theme),
                menu_w,
                menu_h,
                // APPS-9b — baseline the toggle head so a stale pre-launch signal
                // doesn't auto-open the launcher at login.
                last_toggle_ulid: latest_toggle_ulid(),
                // APPS-FX-1 — motion state (idle until an open/hover/tab event).
                anim: Animator::new(),
                hovered: None,
                releasing: None,
                reduce_motion,
                motion: prefs.motion,
                this_host: local_hostname(),
            },
            // Prime the list so the first open is instant.
            load_task(),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        // APPS-9b — watch the bus for a Super-shortcut toggle signal.
        let toggle = cosmic::iced::time::every(Duration::from_millis(TOGGLE_POLL_MS))
            .map(|_| Message::PollToggleSignal);
        // APPS-FX-1 — a ~60 fps animation clock, but **only while a tween is in
        // flight** (open-in / tab crossfade / hover lift). At rest the animator is
        // idle and this subscription isn't created, so there's no idle CPU cost
        // (§7 / MOTION-PERF-1) — motion is strictly event/tick-driven.
        if self.anim.is_idle(Instant::now()) {
            toggle
        } else {
            Subscription::batch([
                toggle,
                cosmic::iced::time::every(Duration::from_millis(ANIM_TICK_MS))
                    .map(|_| Message::AnimTick),
            ])
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
                    // APPS-FX-1 — the popup's surfaces are gone, so no `on_exit`
                    // will arrive for a tile that was hovered at close; clear the
                    // hover state so it doesn't render pre-lifted on the next open.
                    self.reset_hover();
                }
            }
            Message::PollToggleSignal => {
                // APPS-9b — a new toggle ULID since the baseline means a baked
                // Super shortcut fired `--toggle`; flip the dropdown and advance
                // the baseline so the next press toggles again.
                let current = latest_toggle_ulid();
                if should_toggle(self.last_toggle_ulid.as_deref(), current.as_deref()) {
                    self.last_toggle_ulid = current;
                    return self.update(Message::TogglePopup);
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
                // APPS-FX-1 — start from a clean hover slate (a prior close may
                // have skipped `on_exit`), refresh the motion prefs, then start the
                // open-in tween so the dropdown slides up as it appears. A pref
                // change (e.g. reduce-motion toggled) is picked up here, same as
                // the palette refresh below.
                self.reset_hover();
                self.apply_prefs();
                self.start_anim(ANIM_MENU, Motion::panel_mount());
                // APPS-FIT — re-detect the desktop size on open so a resolution
                // change is picked up without a re-login.
                let (mw, mh) = detect_menu_size();
                self.menu_w = mw;
                self.menu_h = mh;
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
                // APPS-FX-1 — refresh prefs (one read) + a clean hover slate, then
                // the power menu slides in like the launcher.
                self.reset_hover();
                self.apply_prefs();
                self.start_anim(ANIM_MENU, Motion::panel_mount());
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
                // APPS-FX-1 — crossfade the result body when the tab actually
                // changes (re-tapping the active tab is a no-op, no flicker).
                if self.tab != t {
                    self.start_anim(ANIM_TAB, Motion::tooltip_fade());
                }
                self.tab = t;
                self.query.clear();
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
            Message::FocusOrLaunchLocal(hint, exec) => {
                // APPS-LIVE-2 — raise the running window instead of relaunching
                // (falls back to relaunch when focus isn't possible), then close.
                focus_or_launch_local(&hint, &exec);
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
                // APPS-FX-1 — one frame: drop settled tweens. When the released
                // element's fall has settled, clear the marker so it's not re-eased.
                let now = Instant::now();
                self.anim.gc(now);
                if let Some(key) = self.releasing.clone() {
                    if !self.anim.is_animating(&key, now) {
                        self.releasing = None;
                    }
                }
            }
            Message::HoverEnter(key) => {
                // APPS-FX-1 — lift/highlight this element; cancel any in-flight
                // release of it.
                if self.releasing.as_deref() == Some(&key) {
                    self.releasing = None;
                }
                if self.hovered.as_deref() != Some(&key) {
                    self.hovered = Some(key.clone());
                    self.start_anim(key, Motion::hover());
                }
            }
            Message::HoverExit(key) => {
                // APPS-FX-1 — settle this element back (only if it's still the
                // hovered one; a stale exit after a fast re-enter is ignored).
                if self.hovered.as_deref() == Some(&key) {
                    self.hovered = None;
                    self.releasing = Some(key.clone());
                    self.start_anim(key, Motion::hover());
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
    /// APPS-FX-1 — re-read the user preferences **once** and refresh everything
    /// derived from them: the active palette (theme), reduce-motion, and the
    /// MOTION-CORE-3 motion controls. One file read per open (the prior code read
    /// `preferences.toml` twice — `resolve_palette` + a separate motion refresh).
    fn apply_prefs(&mut self) {
        let prefs = Preferences::load();
        self.palette = Palette::for_theme(prefs.theme);
        self.reduce_motion = prefs.a11y.reduce_motion;
        self.motion = prefs.motion;
    }

    /// APPS-FX-1 — clear all hover state + its tweens. Called when the popup closes
    /// (no `on_exit` arrives for destroyed surfaces) and on open (so a close path
    /// that bypassed `PopupClosed`, e.g. a launch's `destroy_popup`, can't leave a
    /// tile rendering pre-lifted). Idempotent.
    fn reset_hover(&mut self) {
        self.hovered = None;
        self.releasing = None;
        self.anim.gc(Instant::now());
    }

    /// APPS-FX-1 — start (or restart) the tween under `id` from now, with the
    /// Carbon `preset` resolved against the global motion controls (kill switch +
    /// speed scale) and reduce-motion. Routing every tween through here keeps the
    /// reduce-motion contract + the MOTION-CORE-3 prefs honored in one place.
    /// Also sweeps any settled tweens so the registry can't accumulate a stale
    /// entry once the tick subscription has gone idle.
    fn start_anim(&mut self, id: impl Into<String>, preset: Motion) {
        let now = Instant::now();
        self.anim.gc(now);
        let resolved = self.motion.apply(preset, self.reduce_motion);
        // `apply` already capped/scaled the duration; pass `false` so the
        // animator doesn't re-cap it.
        self.anim.start(id, now, resolved, false);
    }

    /// APPS-FX-1 — the eased open-in transition params (alpha + slide) for the
    /// whole dropdown body at the current frame. A no-op (fully shown) once the
    /// open tween has settled or was never started.
    fn menu_in(&self) -> mde_theme::animation::RenderParams {
        let t = self.anim.value(ANIM_MENU, Instant::now(), Easing::EaseOut);
        Transition::SlideUp(MENU_SLIDE_PX).params(t)
    }

    /// APPS-FX-1 — the tab-switch transition offset (px) for the result body: on a
    /// tab change the body slides up a few px into place (`MENU_SLIDE_PX` → 0),
    /// reading as a crossfade-in without iced 0.13 opacity. `0.0` once settled.
    fn tab_slide(&self) -> f32 {
        Transition::SlideUp(MENU_SLIDE_PX)
            .params(self.anim.value(ANIM_TAB, Instant::now(), Easing::EaseOut))
            .translate_y
    }

    /// APPS-FX-1 — the directional hover "amount" `0.0..=1.0` for the element
    /// `key` ([`hover_key`]) at the current frame: eases **in** (0→1) while
    /// hovered, **out** (1→0) on exit, and is 0 at rest. The single source the
    /// hover-lift (tiles) + hover-highlight (rows) both derive from, so the eased
    /// progress + direction logic lives in one place.
    fn hover_progress(&self, key: &str) -> f32 {
        let t = self.anim.value(key, Instant::now(), Easing::EaseOut);
        if self.hovered.as_deref() == Some(key) {
            t
        } else if self.releasing.as_deref() == Some(key) {
            1.0 - t
        } else {
            0.0
        }
    }

    /// APPS-FX-1 — the hover-lift offset (px, ≤ 0 = up) for a Favorites tile at the
    /// current frame: rises while hovered, settles back on exit. No layout reflow
    /// (rendered as compensating padding — see [`Self::favorite_tile`]).
    fn hover_lift(&self, id: &str) -> f32 {
        let amt = self.hover_progress(&hover_key(HOVER_TILE, id));
        Transition::Lift(TILE_HOVER_RISE_PX).params(amt).translate_y
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

        // ── Header: grid glyph + title, then the QNM-Shared usage line + bar. ──
        let title_row = row(vec![
            text("\u{25A6}\u{FE0E}")
                .size(18)
                .class(cosmic::theme::Text::Color(carbon(p.accent)))
                .into(),
            text("Applications")
                .size(TypeRole::Heading.size_in(sizes))
                .class(cosmic::theme::Text::Color(carbon(p.text)))
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
                .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
                .into(),
            Space::new().width(Length::Fill).into(),
            text(mde_cosmic_applet::qnm_usage_label(self.qnm))
                .size(cap_sz)
                .font(mono)
                .class(cosmic::theme::Text::Color(carbon(p.text)))
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
                            .class(cosmic::theme::Text::Color(carbon(p.text)))
                            .into(),
                        text(label)
                            .size(cap_sz)
                            .class(cosmic::theme::Text::Color(carbon(p.text)))
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
                        cosmic::theme::Text::Color(carbon(if active {
                            p.text
                        } else {
                            p.text_muted
                        })),
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
                .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
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
            self.favorites_grid(&shown)
        } else {
            column(
                shown
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| self.entry_row(i, e))
                    .collect::<Vec<_>>(),
            )
            .spacing(0)
            .width(Length::Fill)
            .into()
        };

        // APPS-FX-1 — tab-switch transition: nudge the result body down a few px
        // and let it settle (top padding `tab_slide` → 0). The offset is applied
        // to the body **inside** the scrollable's content, so it scrolls with the
        // content and never shrinks the viewport — the scroll region keeps its
        // full `Length::Fill` height (no transient clip of the last row).
        let tab_off = self.tab_slide().max(0.0);
        let body = cosmic::iced::widget::container(body).padding(cosmic::iced::Padding {
            top: tab_off,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        });

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

        // APPS-FX-1 — open-in slide: the body starts a few px low and rises to
        // rest (Carbon panel-mount). Rendered as extra top padding that decays to
        // 0 — iced 0.13 has no transform widget, so we offset layout instead
        // (MOTION-INFRA-2's translate-as-padding approach). Bottom padding shrinks
        // by the same amount so the overall height stays put (no jank / reflow of
        // the fixed popup surface).
        let slide = self.menu_in().translate_y.max(0.0);
        let pad = cosmic::iced::Padding {
            top: 12.0 + slide,
            right: 12.0,
            bottom: (12.0 - slide).max(0.0),
            left: 12.0,
        };
        // APPS-FIT — the body fills the detected desktop-fraction size (33% of
        // the screen width × height; falls back to the golden rectangle). Must
        // match the popup positioner size set on open.
        cosmic::iced::widget::container(col)
            .padding(pad)
            .width(Length::Fixed(self.menu_w))
            .height(Length::Fixed(self.menu_h))
            .into()
    }

    /// APPS-WIDE — the primary action for a Favorites tile press: launch apps /
    /// mesh-apps directly (favorites are normally pinned apps), else fall back to
    /// selecting the entry (opens its detail in the list view).
    fn entry_primary_msg(e: &Entry, this_host: &str) -> Message {
        match e.kind.as_str() {
            // APPS-LIVE-2 — a running-here app raises its window instead of
            // relaunching (falls back to relaunch in the handler).
            "app" if !e.exec.is_empty() && app_running_local(e, this_host) => {
                Message::FocusOrLaunchLocal(
                    exec_focus_hint(&e.exec).unwrap_or_default(),
                    e.exec.clone(),
                )
            }
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
    fn favorites_grid(&self, shown: &[&Entry]) -> Element<'static, Message> {
        use cosmic::widget::{column, row, Space};
        let rows: Vec<Element<Message>> = shown
            .iter()
            .take(FAVORITES_MAX)
            .collect::<Vec<_>>()
            .chunks(FAVORITES_COLUMNS)
            .map(|chunk| {
                let mut tiles: Vec<Element<Message>> =
                    chunk.iter().map(|e| self.favorite_tile(e)).collect();
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
    fn favorite_tile(&self, e: &Entry) -> Element<'static, Message> {
        use cosmic::widget::{button, column, mouse_area, text};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let cap_sz = TypeRole::Caption.size_in(sizes);
        // APPS-FX-1 — hover lift (px up) + accent tint for this tile. The icon +
        // label warm to the Carbon accent while hovered (and through the eased
        // settle-back) — a tasteful "this is interactive" cue, not just a colour
        // pop. `lift` is ≤ 0 (up); 0 at rest.
        let lift = self.hover_lift(&e.id);
        let lifting = lift < 0.0;
        let label_c = if lifting { p.accent } else { p.text };
        // APPS-FAV-ICON (operator 2026-06-19) — render the actual Carbon icon
        // SVG (the mde_theme icon set), tinted to the theme text color — the same
        // icons used when docking an app — instead of the Unicode fallback glyph.
        // Falls back to the glyph only if a variant ships no baked SVG.
        let resolved = mde_icon(favorite_icon(e), IconSize::Nav);
        let icon_px = resolved.size_px();
        let icon_widget: Element<'static, Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            let tint = carbon(label_c);
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
                .class(cosmic::theme::Text::Color(carbon(label_c)))
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
                    .class(cosmic::theme::Text::Color(carbon(label_c)))
                    .into(),
            ])
            .spacing(6)
            .align_x(cosmic::iced::Alignment::Center)
            .width(Length::Fill),
        )
        .on_press(Self::entry_primary_msg(e, &self.this_host))
        .width(Length::Fill)
        .class(cosmic::theme::Button::Standard);
        // APPS-FX-1 — render the lift as padding: top reserves `TILE_HOVER_RISE_PX`
        // and is consumed (→0) as the tile rises; bottom grows by the same amount,
        // so the grid cell's height is constant (no neighbour reflow / jank). The
        // `mouse_area` enter/exit events drive the lift tween.
        let top = (TILE_HOVER_RISE_PX + lift).max(0.0);
        let bottom = (-lift).max(0.0);
        let lifted = cosmic::iced::widget::container(tile).padding(cosmic::iced::Padding {
            top,
            right: 0.0,
            bottom,
            left: 0.0,
        });
        let key = hover_key(HOVER_TILE, &e.id);
        mouse_area(lifted)
            .on_enter(Message::HoverEnter(key.clone()))
            .on_exit(Message::HoverExit(key))
            .into()
    }

    /// APPS-STYLE-2 — one result row: letter avatar + accent-blue title + mono
    /// subtitle + status dot, on a zebra layer; clicking toggles the inline
    /// detail ([`Self::detail`]) and the selected row gets a blue left-accent +
    /// raised bg. Theme-aware (all mde-theme tokens).
    fn entry_row<'a>(&self, idx: usize, e: &'a Entry) -> Element<'a, Message> {
        use cosmic::widget::{column, row, text};
        let p = self.palette;
        let sizes = FontSize::defaults();
        let cap_sz = TypeRole::Caption.size_in(sizes);
        let mono = cosmic::iced::Font::MONOSPACE;
        let selected = self.selected.as_deref() == Some(&e.id);
        let sub = match e.kind.as_str() {
            "mesh-app" => format!("mesh · {} · {}", e.node, e.health),
            "workload" => format!("{} · {}", e.source, e.state),
            "service" => format!("service · {}", e.node),
            // APPS-LIVE-1 — a local app stamped running (state="running", node=the
            // host(s) it's live on) reads "<source> · running on <host>".
            "app" if app_running(e) => format!("{} · running on {}", e.source, e.node),
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
                .class(cosmic::theme::Text::Color(carbon(p.text_muted))),
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
                .class(cosmic::theme::Text::Color(carbon(p.text)))
                .into(),
            text(sub)
                .size(cap_sz)
                .font(mono)
                .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
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
        // APPS-FX-1 — hover highlight: an unselected row eases its background
        // toward the raised layer while pointed at (and back out on exit). Pure
        // background blend — the row geometry is untouched, so the list never
        // reflows (no jank). A selected row keeps its own raised treatment.
        let key = hover_key(HOVER_ROW, &e.id);
        let hv = if selected { 0.0 } else { self.hover_progress(&key) };
        // Fast path: a resting/unhovered row keeps the plain zebra color (skip the
        // blend that would just reproduce `shade`) — the common case for a list
        // re-rendered each frame while *some other* row's tween is in flight.
        let bg = if hv == 0.0 {
            carbon(shade)
        } else {
            carbon_mix(shade, p.raised, hv)
        };
        let row_el = cosmic::iced::widget::container(inner)
            .padding([6, 10])
            .width(Length::Fill)
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(bg.into()),
                border: cosmic::iced::Border {
                    color: carbon(accent),
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            });
        cosmic::widget::mouse_area(row_el)
            .on_enter(Message::HoverEnter(key.clone()))
            .on_exit(Message::HoverExit(key))
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
            // APPS-LIVE-1 — a local app gets a running dot only when stamped live.
            "app" if app_running(e) => "running",
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
            "app" if !e.exec.is_empty() && app_running_local(e, &self.this_host) => {
                // APPS-LIVE-2 — running here: the primary action raises the
                // existing window (relaunch is the in-handler fallback).
                actions.push(primary(
                    "Raise",
                    Message::FocusOrLaunchLocal(
                        exec_focus_hint(&e.exec).unwrap_or_default(),
                        e.exec.clone(),
                    ),
                ));
            }
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
mod toggle_tests {
    //! APPS-9b — the Super→launcher toggle plumbing: a `--toggle` publish lands
    //! one `event/apps/toggle` signal the panel applet's poll picks up.
    use super::{should_toggle, write_toggle, TOGGLE_TOPIC};
    use mde_bus::persist::Persist;

    #[test]
    fn should_toggle_only_on_a_new_ulid() {
        // No signal on the bus → never toggles (idle login stays closed).
        assert!(!should_toggle(None, None));
        assert!(!should_toggle(Some("01ABC"), None));
        // A signal newer than the baseline (first press, or a later press) → toggle.
        assert!(should_toggle(None, Some("01ABC")));
        assert!(should_toggle(Some("01ABC"), Some("01XYZ")));
        // The same ULID we already acted on → no repeat toggle (poll is idempotent).
        assert!(!should_toggle(Some("01ABC"), Some("01ABC")));
    }

    #[test]
    fn write_toggle_publishes_a_readable_event_signal() {
        // A fresh store has no toggle event; `--toggle` writes one to the topic,
        // and `latest_ulid` (the applet's poll source) reads exactly it back.
        let dir = std::env::temp_dir().join("mde-apps-toggle-rt-test");
        let _ = std::fs::remove_dir_all(&dir);
        let persist = Persist::open(dir.clone()).expect("open temp bus");
        assert_eq!(persist.latest_ulid(TOGGLE_TOPIC).unwrap(), None);

        let ulid = write_toggle(&persist).expect("publish toggle");
        assert!(!ulid.is_empty());
        assert_eq!(
            persist.latest_ulid(TOGGLE_TOPIC).unwrap().as_deref(),
            Some(ulid.as_str()),
            "the published toggle is the topic head the applet polls"
        );
        // And the applet would flip from its primed baseline (None) to this ULID.
        assert!(should_toggle(None, Some(&ulid)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod motion_tests {
    //! APPS-FX-1 — the launcher's adaptive-budget motion: the pure helpers + the
    //! Carbon-token wiring. The eased interpolation itself is covered by
    //! `mde_theme::animation`; here we pin the launcher-side glue.
    use super::{
        carbon_mix, hover_key, ANIM_HOVER_PREFIX, HOVER_ROW, HOVER_TILE, MENU_SLIDE_PX,
        TILE_HOVER_RISE_PX,
    };
    use mde_theme::animation::{Animator, Transition};
    use mde_theme::{Easing, Motion, Rgba, PANEL_MOUNT_TRANSLATE_Y_PX};
    use std::time::{Duration, Instant};

    #[test]
    fn hover_key_is_prefixed_and_namespaced_per_surface() {
        assert_eq!(
            hover_key(HOVER_TILE, "abc"),
            format!("{ANIM_HOVER_PREFIX}{HOVER_TILE}:abc")
        );
        // Distinct entries differ; the SAME entry on different surfaces also
        // differs — so a favorite tile and a list row never share a tween.
        assert_ne!(hover_key(HOVER_TILE, "a"), hover_key(HOVER_TILE, "b"));
        assert_ne!(hover_key(HOVER_TILE, "a"), hover_key(HOVER_ROW, "a"));
    }

    #[test]
    fn carbon_mix_interpolates_and_clamps() {
        let a = Rgba::rgb(0, 0, 0);
        let b = Rgba::rgb(255, 255, 255);
        // Endpoints return the pure colors.
        let lo = carbon_mix(a, b, 0.0);
        assert!(lo.r.abs() < 1e-6 && lo.g.abs() < 1e-6 && lo.b.abs() < 1e-6);
        let hi = carbon_mix(a, b, 1.0);
        assert!((hi.r - 1.0).abs() < 1e-6);
        // Midpoint blends halfway; out-of-range t is clamped (no overshoot).
        let mid = carbon_mix(a, b, 0.5);
        assert!((mid.r - 0.5).abs() < 1e-3);
        let over = carbon_mix(a, b, 2.0);
        assert!((over.r - 1.0).abs() < 1e-6, "t>1 clamps to b");
    }

    #[test]
    fn slide_tokens_come_from_carbon_panel_mount() {
        // The open-in + tab slide reuse the shared panel-mount translate token —
        // no scattered metric literal (§4).
        assert!((MENU_SLIDE_PX - PANEL_MOUNT_TRANSLATE_Y_PX).abs() < f32::EPSILON);
        // The hover rise is a small, positive component dimension.
        assert!(TILE_HOVER_RISE_PX > 0.0 && TILE_HOVER_RISE_PX <= 8.0);
    }

    #[test]
    fn open_in_slides_from_offset_to_rest() {
        // The dropdown body starts MENU_SLIDE_PX low (extra top padding) and rises
        // to 0 over the panel-mount tween — the visible "appears" motion.
        let t0 = Instant::now();
        let mut a = Animator::new();
        a.start("menu", t0, Motion::panel_mount(), false);
        let at_start = Transition::SlideUp(MENU_SLIDE_PX)
            .params(a.value("menu", t0, Easing::EaseOut))
            .translate_y;
        assert!(at_start > 0.0, "starts below rest, got {at_start}");
        let done = t0 + Motion::panel_mount().duration + Duration::from_millis(1);
        let at_end = Transition::SlideUp(MENU_SLIDE_PX)
            .params(a.value("menu", done, Easing::EaseOut))
            .translate_y;
        assert!(at_end.abs() < 1e-4, "settles at rest, got {at_end}");
    }

    #[test]
    fn animator_goes_idle_so_the_tick_can_stop() {
        // §7 / MOTION-PERF-1 — motion is event/tick-driven: once every tween
        // settles the animator is idle and the applet stops arming the 60fps tick.
        let t0 = Instant::now();
        let mut a = Animator::new();
        a.start(hover_key(HOVER_TILE, "x"), t0, Motion::hover(), false);
        assert!(!a.is_idle(t0), "an in-flight hover ⇒ not idle");
        let done = t0 + Motion::hover().duration + Duration::from_millis(1);
        assert!(a.is_idle(done), "settled ⇒ idle (tick stops)");
    }

    #[test]
    fn reduce_motion_collapses_the_open_tween() {
        // The reduce-motion contract: a disabled-motion / reduce-motion budget
        // caps the open tween to the ≤80 ms Carbon crossfade.
        let prefs = mde_theme::prefs::MotionPrefs::default();
        let resolved = prefs.apply(Motion::panel_mount(), true);
        assert!(resolved.duration <= Duration::from_millis(80));
        // The global kill switch collapses it to a terminal (zero-duration) frame.
        let off = mde_theme::prefs::MotionPrefs {
            enabled: false,
            speed_scale: 1.0,
        };
        assert_eq!(
            off.apply(Motion::panel_mount(), false).duration,
            Duration::ZERO
        );
    }
}
