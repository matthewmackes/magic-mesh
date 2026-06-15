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

struct Center {
    items: Vec<AlertItem>,
    tail: AlertTail,
    /// Source-group labels the operator collapsed.
    collapsed: HashSet<String>,
}

impl Center {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            tail: AlertTail::default(),
            collapsed: HashSet::new(),
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
}

fn subscription(_s: &Center) -> Subscription<Message> {
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
    Subscription::batch([poll, esc])
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

    // Header bar: title + actions.
    let unread = state.items.iter().filter(|i| !i.read).count();
    let title = text(format!("Notification Hub · {unread} unread"))
        .size(16)
        .color(p.text.into_cosmic_color());
    let actions = row![
        action_button("Mark all read", Message::MarkAllRead, p),
        Space::new().width(Length::Fixed(8.0)),
        action_button("Clear all", Message::ClearAll, p),
        Space::new().width(Length::Fixed(8.0)),
        // Close (✕) — also bound to Esc + click-away.
        action_button("✕", Message::Close, p),
    ];
    let header = row![title, Space::new().width(Length::Fill), actions]
        .align_y(cosmic::iced::Alignment::Center);

    let mut body = column![header, Space::new().height(Length::Fixed(8.0))].spacing(6);

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

    container(column![container(scroll).height(Length::Fill), launch_bar,].spacing(0))
        .width(Length::Fill)
        .height(Length::Fill)
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
