//! Keyboard — a thin launcher into the canonical Cosmic input settings
//! (typing / repeat rate / layout), which the desktop applies directly.
//!
//! E0.15 (2026-06-07): the workbench no longer keeps a duplicate libinput
//! surface. The prior in-panel controls persisted `keyboard.*` keys to mackesd
//! Settings and relied on a sway-IPC `input` live-apply — a path retired with
//! the labwc/sway-era desktop. The panel now delegates to that one canonical
//! surface instead of duplicating it.

use std::sync::Arc;

use iced::widget::{column, text};
use iced::{Element, Length, Task};

use crate::backend::Backend;
use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct KeyboardPanel;

/// The launcher has no interactive state of its own — the button fires
/// `crate::Message::OpenSettingsPage` directly — so it carries no sub-message.
/// The empty enum keeps the `app.rs` per-panel routing shape (the dispatch arm
/// is statically unreachable).
#[derive(Debug, Clone)]
pub enum Message {}

impl KeyboardPanel {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// No async state to load — the canonical values live in Cosmic's input settings.
    pub fn load(_backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::none()
    }

    pub fn update(&mut self, message: Message, _backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {}
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        column![
            text("Keyboard").size(18),
            text(
                "Key-repeat delay, repeat rate and layout are configured in \
                 Settings, which applies them to the compositor (Cosmic) directly."
            ),
            variant_button(
                "Open Keyboard settings",
                ButtonVariant::Primary,
                Some(crate::Message::OpenSettingsPage("devices", "typing")),
                crate::live_theme::palette(),
            ),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_builds_a_launcher() {
        let _ = KeyboardPanel::new().view();
    }
}
