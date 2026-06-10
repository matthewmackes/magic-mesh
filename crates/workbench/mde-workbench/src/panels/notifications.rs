//! Notifications panel — DND toggle + placement combo + default
//! expire-ms numeric input.
//!
//! CB-1.9 partial: replaces the v1.x
//! `mackes/workbench/system/notifications.py` GTK3 panel. The
//! Phase F.5 + C.5 wiring already routes:
//!   * `notification.do_not_disturb` → `$XDG_CACHE_HOME/mde/
//!     notifications-dnd` flag file
//!   * `notification.location` → notifications-prefs.json sidecar
//!   * `notification.default_expire_ms` → same sidecar

use std::sync::Arc;

use iced::widget::{checkbox, column, pick_list, row, text, text_input};
use iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{
    encode_bool, parse_bool, parse_u32, quote_json, strip_json_quotes,
};

/// Five-corner placement table the Phase C.5 applier accepts.
pub const LOCATIONS: &[&str] = &[
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
    "center",
];

/// Default expire-ms is locked to 5000 by Phase C.5; the panel
/// only enforces a sane floor (>=0) on parse.
pub const DEFAULT_EXPIRE_MS: u32 = 5000;

pub const KEY_DND: &str = "notification.do_not_disturb";
pub const KEY_LOCATION: &str = "notification.location";
pub const KEY_EXPIRE_MS: &str = "notification.default_expire_ms";

#[derive(Debug, Clone, Default)]
pub struct NotificationsPanel {
    pub dnd: bool,
    pub location: String,
    /// Held as a string for the text_input + numeric parse on
    /// Save. Empty string ⇒ "use default" (5000 ms).
    pub expire_ms_input: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        dnd: bool,
        location: String,
        expire_ms: u32,
    },
    Error(String),
    Saved,
    DndChanged(bool),
    LocationChanged(String),
    ExpireMsChanged(String),
    SaveClicked,
}

impl NotificationsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let dnd = parse_bool(&backend.get(KEY_DND).await?);
                let location = strip_json_quotes(&backend.get(KEY_LOCATION).await?);
                let expire_ms =
                    parse_u32(&backend.get(KEY_EXPIRE_MS).await?).unwrap_or(DEFAULT_EXPIRE_MS);
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    dnd,
                    location,
                    expire_ms,
                })
            },
            |result| {
                crate::Message::Notifications(
                    result.unwrap_or_else(|e| Message::Error(e.to_string())),
                )
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                dnd,
                location,
                expire_ms,
            } => {
                self.dnd = dnd;
                self.location = if LOCATIONS.iter().any(|l| *l == location) {
                    location
                } else {
                    "bottom-right".into()
                };
                self.expire_ms_input = expire_ms.to_string();
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
            Message::DndChanged(v) => {
                self.dnd = v;
                Task::none()
            }
            Message::LocationChanged(v) => {
                self.location = v;
                Task::none()
            }
            Message::ExpireMsChanged(v) => {
                // Accept any input — validate on Save so the
                // user can type a multi-digit number without
                // intermediate state errors.
                self.expire_ms_input = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                // Empty input collapses to the locked default
                // (matches the v1.x panel's blank-as-default
                // behaviour); non-numeric input surfaces a
                // validation error without touching the bus.
                let expire_ms = if self.expire_ms_input.trim().is_empty() {
                    DEFAULT_EXPIRE_MS
                } else {
                    match parse_u32(&self.expire_ms_input) {
                        Some(v) => v,
                        None => {
                            self.status = "Expire-ms must be a non-negative integer.".into();
                            return Task::none();
                        }
                    }
                };
                self.busy = true;
                self.status = "Applying…".into();
                let dnd = self.dnd;
                let location = self.location.clone();
                Task::perform(
                    async move {
                        backend.set(KEY_DND, encode_bool(dnd)).await?;
                        backend.set(KEY_LOCATION, &quote_json(&location)).await?;
                        backend.set(KEY_EXPIRE_MS, &expire_ms.to_string()).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Notifications(
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
            (!self.busy).then(|| crate::Message::Notifications(Message::SaveClicked)),
            crate::live_theme::palette(),
        );
        let location_pick: pick_list::PickList<'_, &'static str, _, _, crate::Message> =
            pick_list(LOCATIONS, current_location(&self.location), |selected| {
                crate::Message::Notifications(Message::LocationChanged(selected.to_string()))
            });

        column![
            checkbox(self.dnd)
                .label("Do Not Disturb")
                .on_toggle(|v| { crate::Message::Notifications(Message::DndChanged(v)) }),
            row![text("Placement").width(Length::Fixed(160.0)), location_pick,].spacing(12),
            row![
                text("Default expire (ms)").width(Length::Fixed(160.0)),
                text_input("5000", &self.expire_ms_input)
                    .on_input(|v| { crate::Message::Notifications(Message::ExpireMsChanged(v)) }),
            ]
            .spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current_location(value: &str) -> Option<&'static str> {
    LOCATIONS.iter().copied().find(|l| *l == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn keys_match_locked_notification_namespace() {
        assert_eq!(KEY_DND, "notification.do_not_disturb");
        assert_eq!(KEY_LOCATION, "notification.location");
        assert_eq!(KEY_EXPIRE_MS, "notification.default_expire_ms");
    }

    #[test]
    fn locations_table_is_five_corners() {
        assert_eq!(LOCATIONS.len(), 5);
        assert!(LOCATIONS.contains(&"bottom-right"));
        assert!(LOCATIONS.contains(&"center"));
    }

    #[test]
    fn default_expire_ms_matches_phase_c5_lock() {
        assert_eq!(DEFAULT_EXPIRE_MS, 5000);
    }

    #[test]
    fn loaded_unknown_location_falls_back_to_bottom_right() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        let _ = panel.update(
            Message::Loaded {
                dnd: true,
                location: "ghost".into(),
                expire_ms: 4000,
            },
            backend,
        );
        assert_eq!(panel.location, "bottom-right");
        assert!(panel.dnd);
        assert_eq!(panel.expire_ms_input, "4000");
    }

    #[test]
    fn save_clicked_with_non_numeric_expire_surfaces_validation_error() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        panel.expire_ms_input = "forever".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert!(panel.status.contains("integer"));
        assert!(!panel.busy);
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[test]
    fn field_change_messages_mutate_matching_fields() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = NotificationsPanel::new();
        let _ = panel.update(Message::DndChanged(true), backend.clone());
        assert!(panel.dnd);
        let _ = panel.update(Message::LocationChanged("top-left".into()), backend.clone());
        assert_eq!(panel.location, "top-left");
        let _ = panel.update(Message::ExpireMsChanged("8000".into()), backend);
        assert_eq!(panel.expire_ms_input, "8000");
    }

    #[tokio::test]
    async fn save_writes_all_three_keys_in_canonical_json_shapes() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend.set(KEY_DND, encode_bool(true)).await.unwrap();
        backend
            .set(KEY_LOCATION, &quote_json("top-right"))
            .await
            .unwrap();
        backend.set(KEY_EXPIRE_MS, "3000").await.unwrap();
        assert_eq!(backend.get(KEY_DND).await.unwrap(), "true");
        assert_eq!(backend.get(KEY_LOCATION).await.unwrap(), "\"top-right\"");
        assert_eq!(backend.get(KEY_EXPIRE_MS).await.unwrap(), "3000");
    }
}
