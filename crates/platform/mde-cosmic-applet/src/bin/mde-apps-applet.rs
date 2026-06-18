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
use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::iced::{Length, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::{Application, Element};

use mde_bus::hooks::config::Priority;
use mde_cosmic_applet::{
    filter_entries, parse_entries, workload_argv, Entry, LauncherTab, WorkloadAction,
};
use mde_theme::{FontSize, Palette, Preferences, Rgba, TypeRole};

const ID: &str = "com.mackes.MagicMeshApps";

/// APPS-STYLE — resolve the active palette from the user's MDE theme preference
/// (`~/.config/mde/preferences.toml`), so the launcher honors **both dark and
/// light** themes (Carbon Gray 100 / Gray 90 / Gray 10) instead of a hardcoded
/// dark palette. Cheap file read; called at init + on each open.
fn resolve_palette() -> Palette {
    Palette::for_theme(Preferences::load().theme)
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
    /// Last load error, shown in the dropdown's empty state.
    error: Option<String>,
    /// APPS-STYLE — the active Carbon palette (dark/light, from the user's MDE
    /// theme preference); refreshed on each open so a theme switch is picked up.
    palette: Palette,
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
    /// Re-fetch the entry list.
    Refresh,
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
                error: None,
                palette: resolve_palette(),
            },
            // Prime the list so the first open is instant.
            load_task(),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
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
                }
            }
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return cosmic::task::message(cosmic::Action::Cosmic(
                        cosmic::app::Action::Surface(destroy_popup(id)),
                    ));
                }
                // Refresh the palette so a theme switch reflects on open.
                self.palette = resolve_palette();
                // Open the dropdown + refresh-on-open (Q: cached + refresh-on-open).
                let open = cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(app_popup::<AppsApplet>(
                        move |state: &mut AppsApplet| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                None,
                                None,
                                None,
                            )
                        },
                        Some(Box::new(move |state: &AppsApplet| {
                            Element::from(state.core.applet.popup_container(state.dropdown()))
                                .map(cosmic::Action::App)
                        })),
                    )),
                ));
                return Task::batch([open, load_task()]);
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
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // Grid/apps glyph (U+FE0E forces monochrome so it honors the Carbon tint).
        let glyph = cosmic::widget::text("\u{25A6}\u{FE0E}") // ▦ apps grid
            .size(16)
            .class(cosmic::theme::Text::Color(carbon(self.palette.text)));
        // APPS-MOUSE-FIX (operator bug 2026-06-18) — the panel button is plain
        // click-to-toggle: `on_press` opens the dropdown, a second press closes
        // it, and a launch closes it (the LaunchLocal/LaunchMesh/OpenService
        // handlers destroy the popup). The old `applet_tooltip` wrapper added a
        // hover subsurface ("mouseover pop up") that interfered with the click —
        // removed; the label lives in the dropdown header instead.
        let btn = cosmic::widget::button::custom(glyph)
            .on_press(Message::TogglePopup)
            .class(cosmic::theme::Button::AppletIcon);
        Element::from(btn)
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
    /// APPS-STYLE-2 — the redesigned Start Menu (design: `docs/design/start-menu-redesign.md`).
    /// Header (title + QNM-Shared usage bar) → quick-link tiles → underline tabs →
    /// search → result rows (zebra + selected blue-accent, click-to-expand detail)
    /// → toast → operator/power footer. 460×720, all Carbon tokens (light + dark).
    fn dropdown(&self) -> Element<'_, Message> {
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
            text("QNM-Shared")
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
        let list: Vec<Element<Message>> = if shown.is_empty() {
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
            vec![cosmic::iced::widget::container(
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
            .into()]
        } else {
            shown
                .into_iter()
                .enumerate()
                .map(|(i, e)| self.entry_row(i, e))
                .collect()
        };

        // ── Assemble: header → links → tabs → search → list (flex) → toast → footer. ──
        let mut col = column(vec![
            header.into(),
            links.into(),
            row(tabs).spacing(0).into(),
            search.into(),
            scrollable(column(list).spacing(0).width(Length::Fill))
                .height(Length::Fill)
                .into(),
        ])
        .spacing(10);
        if let Some(t) = &self.toast {
            col = col.push(self.toast_bar(t));
        }
        col = col.push(self.footer());

        // Golden-ratio portrait: height = width × φ (460 × 1.618 ≈ 744).
        cosmic::iced::widget::container(col)
            .padding(12)
            .width(Length::Fixed(460.0))
            .height(Length::Fixed(744.0))
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
        cosmic::iced::widget::container(inner)
            .padding([6, 10])
            .width(Length::Fill)
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(carbon(shade).into()),
                border: cosmic::iced::Border {
                    color: carbon(accent),
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
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
