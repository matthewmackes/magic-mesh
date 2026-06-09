//! Displays — a thin launcher into the canonical shell display surface.
//! The shell's System ▸ Display Settings page is `mde display`, the dedicated
//! output-configuration surface (resolution / scale / arrangement / night
//! light) that the desktop already ships; this launcher deep-links it via
//! `mde settings system --page display`.
//!
//! E0.15 (2026-06-07): the workbench no longer keeps a duplicate display
//! surface. The prior in-panel controls enumerated outputs via sway IPC
//! (`-t get_outputs`) and persisted `display.*` keys — a path that no-ops under
//! labwc (sway IPC is absent). The panel now delegates to the one canonical
//! surface instead of duplicating it.

use std::sync::Arc;

use iced::widget::{column, text};
use iced::{Element, Length, Task};
use mde_theme::Palette;

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

    /// No async state to load — the canonical surface is `mde display`.
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
                Palette::dark(),
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
