//! Session panel — three booleans controlling MDE session
//! lifecycle behaviour (`session.save_on_exit`,
//! `session.lock_on_suspend`, `session.auto_save`).
//!
//! CB-1.9 partial: replaces the v1.x
//! `mackes/workbench/system/session.py` GTK3 panel. The
//! Phase F.6 sidecar pattern (`~/.cache/mde/session-prefs.json`)
//! stays intact — `mde-session` reads the same file at login.

use std::sync::Arc;

use cosmic::iced::widget::{checkbox, column, row, text};
use cosmic::iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{encode_bool, parse_bool};

pub const KEY_SAVE_ON_EXIT: &str = "session.save_on_exit";
pub const KEY_LOCK_ON_SUSPEND: &str = "session.lock_on_suspend";
pub const KEY_AUTO_SAVE: &str = "session.auto_save";

#[derive(Debug, Clone, Default)]
pub struct SessionPanel {
    pub save_on_exit: bool,
    pub lock_on_suspend: bool,
    pub auto_save: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        save_on_exit: bool,
        lock_on_suspend: bool,
        auto_save: bool,
    },
    Error(String),
    Saved,
    SaveOnExitChanged(bool),
    LockOnSuspendChanged(bool),
    AutoSaveChanged(bool),
    SaveClicked,
}

impl SessionPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let save_on_exit = parse_bool(&backend.get(KEY_SAVE_ON_EXIT).await?);
                let lock_on_suspend = parse_bool(&backend.get(KEY_LOCK_ON_SUSPEND).await?);
                let auto_save = parse_bool(&backend.get(KEY_AUTO_SAVE).await?);
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    save_on_exit,
                    lock_on_suspend,
                    auto_save,
                })
            },
            |result| {
                crate::Message::Session(result.unwrap_or_else(|e| Message::Error(e.to_string())))
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                save_on_exit,
                lock_on_suspend,
                auto_save,
            } => {
                self.save_on_exit = save_on_exit;
                self.lock_on_suspend = lock_on_suspend;
                self.auto_save = auto_save;
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
            Message::SaveOnExitChanged(v) => {
                self.save_on_exit = v;
                Task::none()
            }
            Message::LockOnSuspendChanged(v) => {
                self.lock_on_suspend = v;
                Task::none()
            }
            Message::AutoSaveChanged(v) => {
                self.auto_save = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Applying…".into();
                let save_on_exit = self.save_on_exit;
                let lock_on_suspend = self.lock_on_suspend;
                let auto_save = self.auto_save;
                Task::perform(
                    async move {
                        backend
                            .set(KEY_SAVE_ON_EXIT, &encode_bool(save_on_exit))
                            .await?;
                        backend
                            .set(KEY_LOCK_ON_SUSPEND, &encode_bool(lock_on_suspend))
                            .await?;
                        backend.set(KEY_AUTO_SAVE, &encode_bool(auto_save)).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Session(
                            result.unwrap_or_else(|e| Message::Error(e.to_string())),
                        )
                    },
                )
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        let apply_label = if self.busy { "Applying…" } else { "Apply" };
        // UX-7.a — save routed through the shared button variant.
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::Session(Message::SaveClicked)),
            crate::live_theme::palette(),
        );

        column![
            checkbox(self.save_on_exit)
                .label("Save session on exit")
                .on_toggle(|v| { crate::Message::Session(Message::SaveOnExitChanged(v)) }),
            checkbox(self.lock_on_suspend)
                .label("Lock screen on suspend")
                .on_toggle(|v| crate::Message::Session(Message::LockOnSuspendChanged(v)),),
            checkbox(self.auto_save)
                .label("Auto-save layout periodically")
                .on_toggle(|v| crate::Message::Session(Message::AutoSaveChanged(v)),),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn keys_match_locked_session_namespace() {
        assert_eq!(KEY_SAVE_ON_EXIT, "session.save_on_exit");
        assert_eq!(KEY_LOCK_ON_SUSPEND, "session.lock_on_suspend");
        assert_eq!(KEY_AUTO_SAVE, "session.auto_save");
    }

    #[test]
    fn loaded_clears_busy_and_status() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = SessionPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(
            Message::Loaded {
                save_on_exit: true,
                lock_on_suspend: false,
                auto_save: true,
            },
            backend,
        );
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
        assert!(panel.save_on_exit);
        assert!(panel.auto_save);
        assert!(!panel.lock_on_suspend);
    }

    #[test]
    fn toggle_messages_mutate_matching_field() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = SessionPanel::new();
        let _ = panel.update(Message::SaveOnExitChanged(true), backend.clone());
        assert!(panel.save_on_exit);
        let _ = panel.update(Message::LockOnSuspendChanged(true), backend.clone());
        assert!(panel.lock_on_suspend);
        let _ = panel.update(Message::AutoSaveChanged(true), backend);
        assert!(panel.auto_save);
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = SessionPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[tokio::test]
    async fn save_writes_all_three_keys_as_json_booleans() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend
            .set(KEY_SAVE_ON_EXIT, encode_bool(true))
            .await
            .unwrap();
        backend
            .set(KEY_LOCK_ON_SUSPEND, encode_bool(false))
            .await
            .unwrap();
        backend.set(KEY_AUTO_SAVE, encode_bool(true)).await.unwrap();
        assert_eq!(backend.get(KEY_SAVE_ON_EXIT).await.unwrap(), "true");
        assert_eq!(backend.get(KEY_LOCK_ON_SUSPEND).await.unwrap(), "false");
    }
}
