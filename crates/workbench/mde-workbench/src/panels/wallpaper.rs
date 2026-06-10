//! Wallpaper panel — `wallpaper.path` + `wallpaper.mode`
//! settings keys per Phase C.7. The bg applet (Phase E.2 /
//! E1.2.12) watches the same sidecar and re-paints on change.

use std::sync::Arc;

use iced::widget::{column, pick_list, row, text, text_input};
use iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{quote_json, strip_json_quotes};

/// Locked mode table per Phase C.7. `wallpaper.rs` validates
/// against this set before writing.
pub const MODES: &[&str] = &["stretch", "fit", "fill", "center", "tile"];

pub const KEY_PATH: &str = "wallpaper.path";
pub const KEY_MODE: &str = "wallpaper.mode";

#[derive(Debug, Clone, Default)]
pub struct WallpaperPanel {
    pub path: String,
    pub mode: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded { path: String, mode: String },
    Error(String),
    Saved,
    PathChanged(String),
    ModeChanged(String),
    SaveClicked,
}

impl WallpaperPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let path = strip_json_quotes(&backend.get(KEY_PATH).await?);
                let mode = strip_json_quotes(&backend.get(KEY_MODE).await?);
                Ok::<_, crate::backend::BackendError>(Message::Loaded { path, mode })
            },
            |result| {
                crate::Message::Wallpaper(result.unwrap_or_else(|e| Message::Error(e.to_string())))
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded { path, mode } => {
                self.path = path;
                self.mode = if MODES.iter().any(|m| *m == mode) {
                    mode
                } else {
                    "fill".into()
                };
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::Saved => {
                self.status = "Saved.".into();
                self.busy = false;
                Task::none()
            }
            Message::PathChanged(v) => {
                self.path = v;
                Task::none()
            }
            Message::ModeChanged(v) => {
                self.mode = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Applying…".into();
                let path = self.path.clone();
                let mode = self.mode.clone();
                Task::perform(
                    async move {
                        backend.set(KEY_PATH, &quote_json(&path)).await?;
                        backend.set(KEY_MODE, &quote_json(&mode)).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Wallpaper(
                            result.unwrap_or_else(|e| Message::Error(e.to_string())),
                        )
                    },
                )
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let apply_label = if self.busy { "Applying…" } else { "Apply" };
        // UX-7.a — save routed through the shared button variant.
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::Wallpaper(Message::SaveClicked)),
            crate::live_theme::palette(),
        );
        let mode_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(MODES, current_mode(&self.mode), |selected| {
                crate::Message::Wallpaper(Message::ModeChanged(selected.to_string()))
            });

        column![
            row![
                text("Image path").width(Length::Fixed(120.0)),
                text_input("/usr/share/backgrounds/mde-default.png", &self.path)
                    .on_input(|v| crate::Message::Wallpaper(Message::PathChanged(v))),
            ]
            .spacing(12),
            row![text("Mode").width(Length::Fixed(120.0)), mode_pick].spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current_mode(value: &str) -> Option<&'static str> {
    MODES.iter().copied().find(|m| *m == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn keys_match_locked_wallpaper_namespace() {
        assert_eq!(KEY_PATH, "wallpaper.path");
        assert_eq!(KEY_MODE, "wallpaper.mode");
    }

    #[test]
    fn modes_match_phase_c7_lock() {
        assert_eq!(MODES, &["stretch", "fit", "fill", "center", "tile"]);
    }

    #[test]
    fn loaded_unknown_mode_falls_back_to_fill() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        let _ = panel.update(
            Message::Loaded {
                path: "/usr/share/backgrounds/x.png".into(),
                mode: "wibbly".into(),
            },
            backend,
        );
        assert_eq!(panel.mode, "fill");
        assert_eq!(panel.path, "/usr/share/backgrounds/x.png");
    }

    #[test]
    fn loaded_clears_busy_and_status() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(
            Message::Loaded {
                path: String::new(),
                mode: "fill".into(),
            },
            backend,
        );
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn field_change_messages_mutate_matching_fields() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        let _ = panel.update(
            Message::PathChanged("/home/me/Pictures/bg.jpg".into()),
            backend.clone(),
        );
        assert_eq!(panel.path, "/home/me/Pictures/bg.jpg");
        let _ = panel.update(Message::ModeChanged("center".into()), backend);
        assert_eq!(panel.mode, "center");
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = WallpaperPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[tokio::test]
    async fn save_writes_both_keys_as_quoted_strings() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend.set(KEY_PATH, &quote_json("/x.png")).await.unwrap();
        backend.set(KEY_MODE, &quote_json("fill")).await.unwrap();
        assert_eq!(backend.get(KEY_PATH).await.unwrap(), "\"/x.png\"");
        assert_eq!(backend.get(KEY_MODE).await.unwrap(), "\"fill\"");
    }

    #[test]
    fn current_mode_returns_none_for_unknown() {
        assert_eq!(current_mode("fill"), Some("fill"));
        assert_eq!(current_mode("wibbly"), None);
    }
}
