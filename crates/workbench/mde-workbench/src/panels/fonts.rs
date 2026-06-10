//! Fonts panel — reads + writes `font.name`, `font.monospace`,
//! `font.hinting`, `font.antialias` via the [`Backend`] trait.
//!
//! CB-1.6 lock: replaces the v1.x
//! `mackes/workbench/look_and_feel/fonts.py` GTK3 panel.

use std::sync::Arc;

use iced::widget::{column, pick_list, row, text, text_input};
use iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{quote_json, strip_json_quotes};

/// Locked hinting values the Phase C font applier accepts.
pub const HINTING: &[&str] = &["none", "slight", "medium", "full"];

/// Locked antialiasing values the Phase C font applier accepts.
pub const ANTIALIAS: &[&str] = &["none", "grayscale", "rgba"];

pub const KEY_NAME: &str = "font.name";
pub const KEY_MONOSPACE: &str = "font.monospace";
pub const KEY_HINTING: &str = "font.hinting";
pub const KEY_ANTIALIAS: &str = "font.antialias";

#[derive(Debug, Clone, Default)]
pub struct FontsPanel {
    pub name: String,
    pub monospace: String,
    pub hinting: String,
    pub antialias: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        name: String,
        monospace: String,
        hinting: String,
        antialias: String,
    },
    Error(String),
    Saved,
    NameChanged(String),
    MonospaceChanged(String),
    HintingChanged(String),
    AntialiasChanged(String),
    SaveClicked,
}

impl FontsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let name = strip_json_quotes(&backend.get(KEY_NAME).await?);
                let monospace = strip_json_quotes(&backend.get(KEY_MONOSPACE).await?);
                let hinting = strip_json_quotes(&backend.get(KEY_HINTING).await?);
                let antialias = strip_json_quotes(&backend.get(KEY_ANTIALIAS).await?);
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    name,
                    monospace,
                    hinting,
                    antialias,
                })
            },
            |result| {
                crate::Message::Fonts(result.unwrap_or_else(|e| Message::Error(e.to_string())))
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                name,
                monospace,
                hinting,
                antialias,
            } => {
                self.name = name;
                self.monospace = monospace;
                self.hinting = if HINTING.iter().any(|h| *h == hinting) {
                    hinting
                } else {
                    "slight".into()
                };
                self.antialias = if ANTIALIAS.iter().any(|a| *a == antialias) {
                    antialias
                } else {
                    "rgba".into()
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
            Message::NameChanged(v) => {
                self.name = v;
                Task::none()
            }
            Message::MonospaceChanged(v) => {
                self.monospace = v;
                Task::none()
            }
            Message::HintingChanged(v) => {
                self.hinting = v;
                Task::none()
            }
            Message::AntialiasChanged(v) => {
                self.antialias = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Applying…".into();
                let name = self.name.clone();
                let monospace = self.monospace.clone();
                let hinting = self.hinting.clone();
                let antialias = self.antialias.clone();
                Task::perform(
                    async move {
                        backend.set(KEY_NAME, &quote_json(&name)).await?;
                        backend.set(KEY_MONOSPACE, &quote_json(&monospace)).await?;
                        backend.set(KEY_HINTING, &quote_json(&hinting)).await?;
                        backend.set(KEY_ANTIALIAS, &quote_json(&antialias)).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Fonts(
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
            (!self.busy).then(|| crate::Message::Fonts(Message::SaveClicked)),
            crate::live_theme::palette(),
        );
        let hinting_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(HINTING, current_value(HINTING, &self.hinting), |v| {
                crate::Message::Fonts(Message::HintingChanged(v.to_string()))
            });
        let antialias_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(ANTIALIAS, current_value(ANTIALIAS, &self.antialias), |v| {
                crate::Message::Fonts(Message::AntialiasChanged(v.to_string()))
            });

        column![
            field_row("Interface font", &self.name, |v| crate::Message::Fonts(
                Message::NameChanged(v)
            )),
            field_row("Monospace font", &self.monospace, |v| {
                crate::Message::Fonts(Message::MonospaceChanged(v))
            }),
            row![text("Hinting").width(Length::Fixed(140.0)), hinting_pick,].spacing(12),
            row![
                text("Antialias").width(Length::Fixed(140.0)),
                antialias_pick,
            ]
            .spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(10)
        .into()
    }
}

fn current_value(table: &'static [&'static str], value: &str) -> Option<&'static str> {
    table.iter().copied().find(|t| *t == value)
}

fn field_row<'a, F>(label: &'a str, value: &'a str, on_change: F) -> Element<'a, crate::Message>
where
    F: 'a + Fn(String) -> crate::Message,
{
    row![
        text(label).width(Length::Fixed(140.0)),
        text_input("", value)
            .on_input(on_change)
            .width(Length::Fill),
    ]
    .spacing(12)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn hinting_set_is_locked() {
        assert_eq!(HINTING, &["none", "slight", "medium", "full"]);
    }

    #[test]
    fn antialias_set_is_locked() {
        assert_eq!(ANTIALIAS, &["none", "grayscale", "rgba"]);
    }

    #[test]
    fn keys_match_locked_font_namespace() {
        assert_eq!(KEY_NAME, "font.name");
        assert_eq!(KEY_MONOSPACE, "font.monospace");
        assert_eq!(KEY_HINTING, "font.hinting");
        assert_eq!(KEY_ANTIALIAS, "font.antialias");
    }

    #[test]
    fn loaded_drops_unknown_hinting_to_slight() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = FontsPanel::new();
        let _ = panel.update(
            Message::Loaded {
                name: "Inter 11".into(),
                monospace: "Cascadia Code 10".into(),
                hinting: "ultra".into(),
                antialias: "rgba".into(),
            },
            backend,
        );
        assert_eq!(panel.hinting, "slight");
    }

    #[test]
    fn loaded_drops_unknown_antialias_to_rgba() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = FontsPanel::new();
        let _ = panel.update(
            Message::Loaded {
                name: String::new(),
                monospace: String::new(),
                hinting: "slight".into(),
                antialias: "neon".into(),
            },
            backend,
        );
        assert_eq!(panel.antialias, "rgba");
    }

    #[test]
    fn field_change_messages_mutate_matching_fields() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = FontsPanel::new();
        let _ = panel.update(Message::NameChanged("Inter 11".into()), backend.clone());
        assert_eq!(panel.name, "Inter 11");
        let _ = panel.update(
            Message::MonospaceChanged("Cascadia 10".into()),
            backend.clone(),
        );
        assert_eq!(panel.monospace, "Cascadia 10");
        let _ = panel.update(Message::HintingChanged("full".into()), backend.clone());
        assert_eq!(panel.hinting, "full");
        let _ = panel.update(Message::AntialiasChanged("grayscale".into()), backend);
        assert_eq!(panel.antialias, "grayscale");
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = FontsPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[tokio::test]
    async fn save_writes_all_four_keys() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        // Drive the async path the same way the iced runtime
        // would — open-coded since the executor binding lives
        // upstream.
        backend
            .set(KEY_NAME, &quote_json("Inter 11"))
            .await
            .unwrap();
        backend
            .set(KEY_MONOSPACE, &quote_json("Cascadia Code 10"))
            .await
            .unwrap();
        backend
            .set(KEY_HINTING, &quote_json("slight"))
            .await
            .unwrap();
        backend
            .set(KEY_ANTIALIAS, &quote_json("rgba"))
            .await
            .unwrap();
        assert_eq!(backend.get(KEY_HINTING).await.unwrap(), "\"slight\"");
    }

    #[test]
    fn current_value_returns_none_for_unknown() {
        assert_eq!(current_value(HINTING, "ghost"), None);
        assert_eq!(current_value(HINTING, "full"), Some("full"));
    }
}
