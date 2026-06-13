//! Themes panel — exactly the three Carbon gray themes §4 names
//! (**Gray 10** light · **Gray 90** · **Gray 100** default dark)
//! plus the density scale, read/written through the `mde-theme`
//! preference store (`~/.config/mde/preferences.toml`).
//!
//! GUI-3 (2026-06-09): replaces the retired CB-1.6 GTK panel — the
//! gsettings shell-out, the GTK theme/icon-set/accent free-text
//! fields, and the ChromeOS/Ableton presets are gone. Applying a
//! theme swaps [`crate::live_theme`] (GUI-2) so the whole Workbench
//! repaints live, then persists via [`Preferences::save`].

use cosmic::iced::widget::{column, pick_list, row, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;
use mde_theme::{Density, Preferences, Theme};

use crate::controls::{variant_button, ButtonVariant};

/// The three Carbon gray themes, as (id, label) pairs — §4's full set.
pub const THEMES: &[(&str, &str)] = &[
    ("light", "Gray 10 (light)"),
    ("gray90", "Gray 90"),
    ("dark", "Gray 100 (default dark)"),
];

/// The three density modes (UX-24 — spacing only, never type).
pub const DENSITIES: &[(&str, &str)] = &[
    ("compact", "Compact"),
    ("comfortable", "Comfortable"),
    ("spacious", "Spacious"),
];

/// Panel state — the pending (unapplied) selection + a status line.
#[derive(Debug, Clone, Default)]
pub struct ThemesPanel {
    /// Pending theme id ("light" / "gray90" / "dark").
    pub theme: String,
    /// Pending density id ("compact" / "comfortable" / "spacious").
    pub density: String,
    pub status: String,
}

/// Reducer messages.
#[derive(Debug, Clone)]
pub enum Message {
    /// Initial load — the persisted preferences.
    Loaded {
        theme: String,
        density: String,
    },
    ThemePicked(String),
    DensityPicked(String),
    /// Apply: swap the live theme + persist.
    ApplyClicked,
    /// Persist outcome.
    Saved(Result<(), String>),
}

/// Map a density id to the enum. (Density has no id round-trip in
/// mde-theme yet; the mapping lives here with the only consumer.)
fn density_from_id(s: &str) -> Option<Density> {
    match s {
        "compact" => Some(Density::Compact),
        "comfortable" => Some(Density::Comfortable),
        "spacious" => Some(Density::Spacious),
        _ => None,
    }
}

fn density_id(d: Density) -> &'static str {
    match d {
        Density::Compact => "compact",
        Density::Comfortable => "comfortable",
        Density::Spacious => "spacious",
    }
}

impl ThemesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Initial load — reads the persisted preferences (no backend;
    /// the pref store is the source of truth, GUI-3).
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                let p = Preferences::load();
                Message::Loaded {
                    theme: p.theme.id().to_string(),
                    density: density_id(p.density).to_string(),
                }
            },
            crate::Message::Themes,
        )
    }

    /// Apply a reducer message.
    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded { theme, density } => {
                self.theme = theme;
                self.density = density;
                self.status.clear();
                Task::none()
            }
            Message::ThemePicked(v) => {
                self.theme = v;
                Task::none()
            }
            Message::DensityPicked(v) => {
                self.density = v;
                Task::none()
            }
            Message::ApplyClicked => {
                let Some(theme) = Theme::from_id(&self.theme) else {
                    self.status = format!("unknown theme id: {}", self.theme);
                    return Task::none();
                };
                let Some(density) = density_from_id(&self.density) else {
                    self.status = format!("unknown density id: {}", self.density);
                    return Task::none();
                };
                // GUI-2 — swap the live tokens; the whole app repaints
                // on the next render pass.
                crate::live_theme::set(theme, density);
                self.status = "Applied.".to_string();
                // Persist (a11y prefs carried through untouched).
                Task::perform(
                    async move {
                        let mut p = Preferences::load();
                        p.theme = theme;
                        p.density = density;
                        Message::Saved(p.save().map_err(|e| e.to_string()))
                    },
                    crate::Message::Themes,
                )
            }
            Message::Saved(result) => {
                self.status = match result {
                    Ok(()) => "Applied + saved.".to_string(),
                    Err(e) => format!("applied (live) but not saved: {e}"),
                };
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();

        let theme_labels: Vec<&'static str> = THEMES.iter().map(|(_, l)| *l).collect();
        let theme_selected = THEMES
            .iter()
            .find(|(id, _)| *id == self.theme)
            .map(|(_, l)| *l);
        let theme_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message, cosmic::Theme> =
            pick_list(theme_labels, theme_selected, |label| {
                let id = THEMES
                    .iter()
                    .find(|(_, l)| *l == label)
                    .map_or("dark", |(id, _)| *id);
                crate::Message::Themes(Message::ThemePicked(id.to_string()))
            });

        let density_labels: Vec<&'static str> = DENSITIES.iter().map(|(_, l)| *l).collect();
        let density_selected = DENSITIES
            .iter()
            .find(|(id, _)| *id == self.density)
            .map(|(_, l)| *l);
        let density_pick: pick_list::PickList<
            '_,
            &'static str,
            _,
            _,
            crate::Message,
            cosmic::Theme,
        > = pick_list(density_labels, density_selected, |label| {
            let id = DENSITIES
                .iter()
                .find(|(_, l)| *l == label)
                .map_or("comfortable", |(id, _)| *id);
            crate::Message::Themes(Message::DensityPicked(id.to_string()))
        });

        let apply_btn = variant_button(
            "Apply",
            ButtonVariant::Primary,
            Some(crate::Message::Themes(Message::ApplyClicked)),
            palette,
        );

        // PLANES-2 — Look & Feel rides on COSMIC; carry its hero.
        let cosmic = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Cosmic,
            crate::panel_chrome::pkg_version_cached("cosmic-comp").as_deref(),
            palette,
        );
        column![
            row![
                text("Appearance").size(20),
                cosmic::iced::widget::Space::new().width(Length::Fill),
                cosmic,
            ]
            .align_y(cosmic::iced::Alignment::Center),
            row![text("Theme").width(Length::Fixed(120.0)), theme_pick].spacing(12),
            row![text("Density").width(Length::Fixed(120.0)), density_pick].spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(10)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_are_locked_to_the_three_carbon_grays() {
        // §4 — exactly Gray 10 / Gray 90 / Gray 100, nothing else.
        let ids: Vec<&str> = THEMES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["light", "gray90", "dark"]);
        for (id, _) in THEMES {
            assert!(Theme::from_id(id).is_some(), "id {id} must parse");
        }
    }

    #[test]
    fn densities_round_trip() {
        for (id, _) in DENSITIES {
            let d = density_from_id(id).expect("known id");
            assert_eq!(density_id(d), *id);
        }
        assert!(density_from_id("enormous").is_none());
    }

    #[test]
    fn apply_with_unknown_theme_reports_not_panics() {
        let mut panel = ThemesPanel {
            theme: "sepia".into(),
            density: "comfortable".into(),
            status: String::new(),
        };
        let _ = panel.update(Message::ApplyClicked);
        assert!(panel.status.contains("unknown theme"));
    }

    #[test]
    fn apply_swaps_the_live_palette() {
        let mut panel = ThemesPanel {
            theme: "gray90".into(),
            density: "comfortable".into(),
            status: String::new(),
        };
        let _ = panel.update(Message::ApplyClicked);
        assert_eq!(
            crate::live_theme::palette().background,
            mde_theme::Palette::gray_90().background
        );
        // Restore the default for other tests sharing the global.
        crate::live_theme::set(Theme::Dark, Density::Comfortable);
    }
}
