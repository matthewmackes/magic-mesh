//! Displays — a thin launcher into the canonical display-settings surface.
//! Resolution / scale / arrangement / night light are configured in the
//! Cosmic display settings; this panel shows a single call-to-action button
//! that opens that surface rather than duplicating the controls.
//!
//! E0.15 (2026-06-07): the workbench no longer keeps a duplicate display
//! surface. The prior in-panel controls enumerated outputs via sway IPC
//! (`-t get_outputs`) and persisted `display.*` keys — a path retired with the
//! labwc/sway-era desktop. The panel now delegates to the one canonical
//! surface instead of duplicating it.

use std::sync::Arc;

use iced::widget::{column, text};
use iced::{Element, Length, Task};

use crate::backend::Backend;
use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct DisplaysPanel;

/// The launcher has no interactive state of its own — the button fires
/// `crate::Message::OpenSettingsPage` directly — so it carries no sub-message.
/// The empty enum keeps the `app.rs` per-panel routing shape (the dispatch arm
/// is statically unreachable).
#[derive(Debug, Clone)]
pub enum Message {}

impl DisplaysPanel {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// No async state to load — the canonical surface is the Cosmic
    /// display settings the button opens.
    pub fn load(_backend: Arc<dyn Backend>) -> Task<crate::Message> {
        Task::none()
    }

    pub fn update(&mut self, message: Message, _backend: Arc<dyn Backend>) -> Task<crate::Message> {
        match message {}
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        column![
            text("Displays").size(18),
            text(
                "Resolution, scale, arrangement and night light are configured in \
                 Settings ▸ Display, which applies them to the compositor (labwc) \
                 directly."
            ),
            variant_button(
                "Open Display settings",
                ButtonVariant::Primary,
                Some(crate::Message::OpenSettingsPage("system", "display")),
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
        let _ = DisplaysPanel::new().view();
    }
}
