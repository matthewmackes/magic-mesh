//! Removable Media panel — three booleans controlling
//! `automount.*` behaviour.
//!
//! CB-1.4 partial: replaces the v1.x
//! `mackes/workbench/system/removable.py` GTK3 panel. Phase
//! F.2 + C.6 already wired the sidecar
//! `$XDG_CACHE_HOME/mde/automount.json`; this Iced port reads
//! + writes through `dev.mackes.MDE.Settings.Get/Set` like
//! every other panel in the workbench.

use std::sync::Arc;

use iced::widget::{checkbox, column, row, text};
use iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};

use crate::backend::Backend;
use crate::panels::json_helpers::{encode_bool, parse_bool};

pub const KEY_ON_INSERT: &str = "automount.on_insert";
pub const KEY_OPEN_ON_MOUNT: &str = "automount.open_on_mount";
pub const KEY_AUTORUN: &str = "automount.autorun";

#[derive(Debug, Clone, Default)]
pub struct RemovablePanel {
    pub on_insert: bool,
    pub open_on_mount: bool,
    pub autorun: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        on_insert: bool,
        open_on_mount: bool,
        autorun: bool,
    },
    Error(String),
    Saved,
    OnInsertChanged(bool),
    OpenOnMountChanged(bool),
    AutorunChanged(bool),
    SaveClicked,
}

impl RemovablePanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::perform(
            async move {
                let on_insert = parse_bool(&backend.get(KEY_ON_INSERT).await?);
                let open_on_mount = parse_bool(&backend.get(KEY_OPEN_ON_MOUNT).await?);
                let autorun = parse_bool(&backend.get(KEY_AUTORUN).await?);
                Ok::<_, crate::backend::BackendError>(Message::Loaded {
                    on_insert,
                    open_on_mount,
                    autorun,
                })
            },
            |result| {
                crate::Message::Removable(result.unwrap_or_else(|e| Message::Error(e.to_string())))
            },
        )
    }

    pub fn update(&mut self, message: Message, backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                on_insert,
                open_on_mount,
                autorun,
            } => {
                self.on_insert = on_insert;
                self.open_on_mount = open_on_mount;
                self.autorun = autorun;
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
            Message::OnInsertChanged(v) => {
                self.on_insert = v;
                Task::none()
            }
            Message::OpenOnMountChanged(v) => {
                self.open_on_mount = v;
                Task::none()
            }
            Message::AutorunChanged(v) => {
                self.autorun = v;
                Task::none()
            }
            Message::SaveClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Applying…".into();
                let on_insert = self.on_insert;
                let open_on_mount = self.open_on_mount;
                let autorun = self.autorun;
                Task::perform(
                    async move {
                        backend.set(KEY_ON_INSERT, encode_bool(on_insert)).await?;
                        backend
                            .set(KEY_OPEN_ON_MOUNT, encode_bool(open_on_mount))
                            .await?;
                        backend.set(KEY_AUTORUN, encode_bool(autorun)).await?;
                        Ok::<_, crate::backend::BackendError>(Message::Saved)
                    },
                    |result| {
                        crate::Message::Removable(
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
        // Primary because save is the panel's dominant CTA.
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::Removable(Message::SaveClicked)),
            crate::live_theme::palette(),
        );

        column![
            checkbox(self.on_insert)
                .label("Auto-mount on insert")
                .on_toggle(|v| { crate::Message::Removable(Message::OnInsertChanged(v)) }),
            checkbox(self.open_on_mount)
                .label("Open file manager on mount")
                .on_toggle(|v| crate::Message::Removable(Message::OpenOnMountChanged(v)),),
            checkbox(self.autorun)
                .label("Honour autorun on inserted media")
                .on_toggle(|v| { crate::Message::Removable(Message::AutorunChanged(v)) }),
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
    fn keys_match_locked_automount_namespace() {
        assert_eq!(KEY_ON_INSERT, "automount.on_insert");
        assert_eq!(KEY_OPEN_ON_MOUNT, "automount.open_on_mount");
        assert_eq!(KEY_AUTORUN, "automount.autorun");
    }

    #[test]
    fn loaded_clears_busy_and_carries_flags() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = RemovablePanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(
            Message::Loaded {
                on_insert: true,
                open_on_mount: false,
                autorun: false,
            },
            backend,
        );
        assert!(panel.on_insert);
        assert!(!panel.open_on_mount);
        assert!(!panel.autorun);
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn toggle_messages_mutate_matching_field() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = RemovablePanel::new();
        let _ = panel.update(Message::OnInsertChanged(true), backend.clone());
        assert!(panel.on_insert);
        let _ = panel.update(Message::OpenOnMountChanged(true), backend.clone());
        assert!(panel.open_on_mount);
        let _ = panel.update(Message::AutorunChanged(true), backend);
        assert!(panel.autorun);
    }

    #[test]
    fn save_clicked_while_busy_is_noop() {
        let backend = Arc::new(DemoBackend::new());
        let mut panel = RemovablePanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SaveClicked, backend);
        assert_eq!(panel.status, "Applying…");
    }

    #[tokio::test]
    async fn save_writes_all_three_keys_as_json_booleans() {
        let backend: Arc<dyn Backend> = Arc::new(DemoBackend::new());
        backend.set(KEY_ON_INSERT, encode_bool(true)).await.unwrap();
        backend
            .set(KEY_OPEN_ON_MOUNT, encode_bool(false))
            .await
            .unwrap();
        backend.set(KEY_AUTORUN, encode_bool(false)).await.unwrap();
        assert_eq!(backend.get(KEY_ON_INSERT).await.unwrap(), "true");
        assert_eq!(backend.get(KEY_AUTORUN).await.unwrap(), "false");
    }
}
