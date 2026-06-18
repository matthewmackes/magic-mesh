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
use mde_cosmic_applet::{filter_entries, parse_entries, Entry, LauncherTab};
use mde_theme::{FontSize, Palette, Rgba, TypeRole};

const ID: &str = "com.mackes.MagicMeshApps";

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
    /// Last load error, shown in the dropdown's empty state.
    error: Option<String>,
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
    /// Re-fetch the entry list.
    Refresh,
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
                error: None,
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
            Message::Refresh => return load_task(),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // Grid/apps glyph (U+FE0E forces monochrome so it honors the Carbon tint).
        let glyph = cosmic::widget::text("\u{25A6}\u{FE0E}") // ▦ apps grid
            .size(16)
            .class(cosmic::theme::Text::Color(carbon(Palette::dark().text)));
        let btn = cosmic::widget::button::custom(glyph)
            .on_press(Message::TogglePopup)
            .class(cosmic::theme::Button::AppletIcon);
        Element::from(self.core.applet.applet_tooltip::<Message>(
            btn,
            "Applications — launch anything in the mesh".to_string(),
            self.popup.is_some(),
            |_| Message::TogglePopup,
            None,
        ))
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
    /// Render the dropdown body: tab row → search → filtered entry list.
    fn dropdown(&self) -> Element<'_, Message> {
        use cosmic::widget::{button, column, row, scrollable, text, text_input};
        let p = Palette::dark();
        let sizes = FontSize::defaults();
        let body_sz = TypeRole::Body.size_in(sizes);
        let cap_sz = TypeRole::Caption.size_in(sizes);

        // APPS-3 header: live QNM-Shared usage (Q8) + quick links (Q4). The links
        // launch detached, like any local app.
        let disk = text(mde_cosmic_applet::qnm_usage_label(self.qnm))
            .size(cap_sz)
            .class(cosmic::theme::Text::Color(carbon(p.text_muted)));
        let quick = |label: &'static str, exec: &'static str| -> Element<Message> {
            button::custom(text(label).size(cap_sz))
                .on_press(Message::LaunchLocal(exec.to_string()))
                .class(cosmic::theme::Button::Text)
                .into()
        };
        let links = row(vec![
            quick("Workbench", "mde-workbench"),
            quick("Files", "mde-files"),
            quick("Settings", "cosmic-settings"),
        ])
        .spacing(4);
        let header = column(vec![disk.into(), links.into()]).spacing(2);

        // Tab row (Favorites first — Q6). The active tab is accented.
        let tabs: Vec<Element<Message>> = LauncherTab::all()
            .into_iter()
            .map(|t| {
                let active = t == self.tab && self.query.trim().is_empty();
                let lbl = text(t.label())
                    .size(body_sz)
                    .class(cosmic::theme::Text::Color(carbon(if active {
                        p.accent
                    } else {
                        p.text_muted
                    })));
                button::custom(lbl)
                    .on_press(Message::SetTab(t))
                    .class(if active {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
                    .into()
            })
            .collect();

        let search = text_input("Search apps, mesh, services…", &self.query)
            .on_input(Message::Search)
            .width(Length::Fill);

        // Filtered entries for the active tab (or the cross-tab search).
        let shown = filter_entries(&self.entries, self.tab, &self.query, &self.favorites);
        let list: Vec<Element<Message>> = if shown.is_empty() {
            let msg = if let Some(e) = &self.error {
                format!("Couldn't reach the apps service: {e}")
            } else if self.tab == LauncherTab::Favorites && self.query.trim().is_empty() {
                "No favorites yet — pin apps to see them here.".to_string()
            } else {
                "Nothing here.".to_string()
            };
            vec![text(msg)
                .size(cap_sz)
                .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
                .into()]
        } else {
            shown.into_iter().map(|e| self.entry_row(e)).collect()
        };

        column(vec![
            header.into(),
            row(tabs).spacing(4).into(),
            search.into(),
            scrollable(column(list).spacing(2).width(Length::Fill))
                .height(Length::Fixed(420.0))
                .into(),
        ])
        .spacing(8)
        .width(Length::Fixed(340.0))
        .into()
    }

    /// One entry row — a pin toggle (APPS-4) + name/hint launch button. Local
    /// apps launch on click; other kinds render (launch paths land in APPS-5/6/7).
    fn entry_row<'a>(&self, e: &'a Entry) -> Element<'a, Message> {
        use cosmic::widget::{button, column, row, text};
        let p = Palette::dark();
        let sizes = FontSize::defaults();
        let cap_sz = TypeRole::Caption.size_in(sizes);
        let sub = match e.kind.as_str() {
            "mesh-app" => format!("mesh · {} · {}", e.node, e.health),
            "workload" => format!("{} · {}", e.source, e.state),
            "service" => format!("service · {}", e.node),
            _ => e.source.clone(),
        };
        // Pin toggle (★ pinned / ☆ not) — persists mesh-wide (APPS-4).
        let pinned = self.favorites.contains(&e.id);
        let star = button::custom(
            text(if pinned { "\u{2605}" } else { "\u{2606}" })
                .size(cap_sz)
                .class(cosmic::theme::Text::Color(carbon(if pinned {
                    p.accent
                } else {
                    p.text_muted
                }))),
        )
        .on_press(Message::ToggleFavorite(e.id.clone()))
        .class(cosmic::theme::Button::Text);

        let body = column(vec![
            text(e.name.clone())
                .size(TypeRole::Body.size_in(sizes))
                .into(),
            text(sub)
                .size(cap_sz)
                .class(cosmic::theme::Text::Color(carbon(p.text_muted)))
                .into(),
        ])
        .spacing(1);
        let launch = button::custom(body)
            .width(Length::Fill)
            .class(cosmic::theme::Button::Text);
        let launch = match e.kind.as_str() {
            // Local apps exec directly (Q23); mesh peers open a remote-desktop
            // session (APPS-5) — launchable even when degraded (Q18). Workload +
            // service launch land in APPS-6/7.
            "app" if !e.exec.is_empty() => launch.on_press(Message::LaunchLocal(e.exec.clone())),
            "mesh-app" if !e.node.is_empty() => {
                launch.on_press(Message::LaunchMesh(e.node.clone()))
            }
            _ => launch,
        };
        row(vec![star.into(), launch.into()]).spacing(4).into()
    }
}
