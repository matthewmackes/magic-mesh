//! v4.0.1 WB-2.d — Apps → Panel Apps editor.
//!
//! Per-applet visibility toggles for the mde-panel tray. Reads
//! `~/.config/mde/panel.toml` (via `mackes_config::PanelConfig`),
//! shows one checkbox per well-known tray applet (audio,
//! network, mesh, status, clipboard, notifications), and writes
//! changes back. `mde-panel`'s `top_bar.rs` consults the same
//! TOML on launch to decide which tray buttons to render.
//!
//! The panel.toml schema (`mackes_config::PanelConfig::top_bar
//! ::status_items: Vec<String>`) already supports an ordered
//! list; this panel toggles entries in/out of that list. Old
//! `mackes-panel/panel.toml` paths are also probed for
//! back-compat read; writes always go to the new MDE-namespaced
//! location.
//!
//! Chrome influence: Win11 Settings → Personalization → Taskbar
//! → Taskbar items toggles.

use std::path::PathBuf;

use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

/// Well-known applet ids the operator can toggle. Locked in
/// code (not in `panel.toml`) — adding a new applet means a
/// code change here + a tray-button arm in mde-panel's
/// `top_bar.rs`. The display label + Material Symbols icon track those
/// arms.
pub const APPLETS: &[(&str, &str, Icon)] = &[
    ("audio", "Audio", Icon::Sound),
    ("network", "Network", Icon::Network),
    ("mesh", "Mesh", Icon::Fleet),
    ("status", "Status cluster", Icon::Inventory),
    ("clipboard", "Clipboard", Icon::Logs),
    ("notifications", "Notifications", Icon::Notification),
];

#[derive(Debug, Clone, Default)]
pub struct PanelAppsPanel {
    pub visible: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<String>),
    Toggle(String),
    Saved(Result<(), String>),
}

impl PanelAppsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { load_visible_applets() }, |visible| {
            crate::Message::PanelApps(Message::Loaded(visible))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(v) => {
                self.visible = v;
                self.status = format!("{} applets visible", self.visible.len());
                Task::none()
            }
            Message::Toggle(id) => {
                if let Some(pos) = self.visible.iter().position(|s| s == &id) {
                    self.visible.remove(pos);
                } else {
                    self.visible.push(id);
                }
                let to_save = self.visible.clone();
                Task::perform(async move { save_visible_applets(&to_save) }, |r| {
                    crate::Message::PanelApps(Message::Saved(r))
                })
            }
            Message::Saved(Ok(())) => {
                self.status = format!(
                    "saved · {} applets visible · restart mde-panel to apply",
                    self.visible.len()
                );
                Task::none()
            }
            Message::Saved(Err(e)) => {
                self.status = format!("save failed: {e}");
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = Palette::dark();
        let sizes = FontSize::defaults();

        let title = text("Panel Apps")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let subtitle_text = if self.status.is_empty() {
            "toggle which applets render in the panel's tray zone".to_string()
        } else {
            self.status.clone()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let header = column![title, subtitle].spacing(2);

        let mut col = column![].spacing(8);
        for (id, label, icon) in APPLETS {
            let on = self.visible.iter().any(|s| s == id);
            col = col.push(applet_row(id, label, *icon, on, palette));
        }

        let footer = text(
            "Changes save to ~/.config/mde/panel.toml; restart mde-panel (or rerun `restart-panel-stack.sh panel`) to apply.",
        )
        .size(10)
        .color(palette.text_muted.into_iced_color());

        container(
            column![
                header,
                Space::new().height(Length::Fixed(18.0)),
                col,
                Space::new().height(Length::Fixed(16.0)),
                footer,
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn applet_row<'a>(
    id: &'a str,
    label: &'a str,
    icon: Icon,
    on: bool,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let resolved = mde_icon(icon, IconSize::Inline);
    let icon_color = if on {
        palette.accent.into_iced_color()
    } else {
        palette.text_muted.into_iced_color()
    };
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(18.0)
            .color(icon_color)
            .into()
    };

    let label_text = text(label.to_string())
        .size(14)
        .color(palette.text.into_iced_color());
    let id_text = text(format!("({id})"))
        .size(11)
        .color(palette.text_muted.into_iced_color());

    let toggle_label = if on { "ON" } else { "OFF" };
    let accent = palette.accent.into_iced_color();
    let muted = palette.text_muted.into_iced_color();
    let id_owned = id.to_string();
    let toggle_btn = button(text(toggle_label).size(11).color(if on {
        Color::WHITE
    } else {
        palette.text_muted.into_iced_color()
    }))
    .padding(Padding::from([4u16, 14u16]))
    .style(move |_t: &Theme, status: iced::widget::button::Status| {
        let bg = if on {
            match status {
                iced::widget::button::Status::Hovered => Color {
                    r: accent.r * 1.10,
                    g: accent.g * 1.10,
                    b: accent.b * 1.10,
                    a: accent.a,
                },
                _ => accent,
            }
        } else {
            match status {
                iced::widget::button::Status::Hovered => Color {
                    r: 0.15,
                    g: 0.15,
                    b: 0.17,
                    a: 1.0,
                },
                _ => Color::TRANSPARENT,
            }
        };
        iced::widget::button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color: if on { Color::WHITE } else { muted },
            border: Border {
                color: if on {
                    Color::TRANSPARENT
                } else {
                    Color {
                        a: 0.20,
                        ..Color::WHITE
                    }
                },
                width: if on { 0.0 } else { 1.0 },
                radius: 4.0.into(),
            },
            shadow: iced::Shadow::default(),
        }
    })
    .on_press(crate::Message::PanelApps(Message::Toggle(id_owned)));

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(
        row![
            icon_widget,
            column![label_text, id_text].spacing(2),
            Space::new().width(Length::Fill),
            toggle_btn,
        ]
        .spacing(12)
        .align_y(iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 16u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 5.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

// ---- I/O ------------------------------------------------------

/// Resolve `~/.config/mde/panel.toml` first, fall back to the
/// legacy `~/.config/mackes-panel/panel.toml`. Returns the
/// merged status_items list, OR the locked default when no
/// file exists yet.
#[must_use]
pub fn load_visible_applets() -> Vec<String> {
    let candidates = config_paths();
    for path in &candidates {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(cfg) = mackes_config::parse(&raw) {
                return cfg.top_bar.status_items;
            }
        }
    }
    default_visible_applets()
}

/// All six well-known applets visible by default (matches
/// mde-panel's tray order when no config is present).
#[must_use]
pub fn default_visible_applets() -> Vec<String> {
    APPLETS.iter().map(|(id, _, _)| id.to_string()).collect()
}

/// Write `visible` to the MDE config path (always — never
/// touches the legacy mackes-panel location). Creates the
/// parent dir if missing.
pub fn save_visible_applets(visible: &[String]) -> Result<(), String> {
    let path = primary_config_path().ok_or_else(|| "no $HOME".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    // Load current cfg (may be the default), patch status_items,
    // round-trip back to TOML so other sections survive.
    let mut cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| mackes_config::parse(&raw).ok())
        .unwrap_or_else(mackes_config::default_config);
    cfg.top_bar.status_items = visible.to_vec();
    let text = mackes_config::to_toml_string(&cfg).map_err(|e| format!("encode: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn primary_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("mde").join("panel.toml"))
}

fn config_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = primary_config_path() {
        out.push(p);
    }
    if let Some(home) = std::env::var_os("HOME") {
        out.push(
            PathBuf::from(home)
                .join(".config")
                .join("mackes-panel")
                .join("panel.toml"),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applet_list_is_six_locked() {
        // Locked at WB-2.d ship: six well-known applets. Adding
        // a 7th means coordinating with mde-panel/top_bar.rs.
        assert_eq!(APPLETS.len(), 6);
    }

    #[test]
    fn default_visible_lists_every_applet() {
        let v = default_visible_applets();
        assert_eq!(v.len(), APPLETS.len());
        for (id, _, _) in APPLETS {
            assert!(v.contains(&id.to_string()));
        }
    }

    #[test]
    fn toggle_adds_when_absent_removes_when_present() {
        let mut p = PanelAppsPanel::new();
        p.visible = vec!["audio".into(), "network".into()];
        // Toggle off audio.
        let _ = p.update(Message::Toggle("audio".into()));
        assert!(!p.visible.contains(&"audio".into()));
        assert!(p.visible.contains(&"network".into()));
        // Toggle audio back on.
        let _ = p.update(Message::Toggle("audio".into()));
        assert!(p.visible.contains(&"audio".into()));
    }

    #[test]
    fn save_round_trips_through_toml() {
        // Run only when $HOME is set (skips in headless CI
        // containers without a writable home).
        let Some(path) = primary_config_path() else {
            return;
        };
        if path.exists() {
            // Don't clobber a real config file.
            return;
        }
        let visible = vec!["audio".into(), "mesh".into()];
        if save_visible_applets(&visible).is_ok() {
            let loaded = load_visible_applets();
            // Loaded list contains what we wrote (could be
            // identical or default-merged with extras).
            assert!(loaded.contains(&"audio".into()));
            assert!(loaded.contains(&"mesh".into()));
            // Clean up.
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn view_renders_without_panic() {
        let p = PanelAppsPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_with_loaded_data_renders_without_panic() {
        let mut p = PanelAppsPanel::new();
        p.visible = vec!["audio".into(), "network".into(), "mesh".into()];
        let _ = p.view();
    }
}
