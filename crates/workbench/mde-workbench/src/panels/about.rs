//! E6.9 — Help → About: the single-source project disclaimer/mission.
//!
//! Renders [`mde_disclaimer::TEXT`] — embedded from the repo-root
//! `DISCLAIMER.md` at build time via `include_str!`, never copy-pasted —
//! so the Workbench shows the exact same Warning / Disclaimer / Mission
//! text as the shell About, the installer, and the daemon banner. Edit
//! `DISCLAIMER.md` and every consumer updates on the next build.

use iced::widget::{column, container, scrollable, text, Space};
use iced::{Element, Length, Padding};
use mde_theme::{FontSize, TypeRole};

#[derive(Debug, Clone, Copy, Default)]
pub struct AboutPanel;

impl AboutPanel {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The canonical disclaimer text this panel renders — the single
    /// source ([`mde_disclaimer::TEXT`]), exposed so a test can assert
    /// the surface never drifts from `DISCLAIMER.md` (E6.9 acceptance #2).
    #[must_use]
    pub const fn disclaimer_text() -> &'static str {
        mde_disclaimer::TEXT
    }

    pub fn view<'a>() -> Element<'a, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();
        let title = text("About Mackes Workstation")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        // The full canonical text in one block so the rendered copy
        // matches the file exactly (no re-splitting / re-wording).
        let body = text(Self::disclaimer_text())
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text.into_iced_color());
        let col = column![title, Space::new().height(Length::Fixed(12.0)), body].spacing(0);
        container(scrollable(col))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_without_panic() {
        let _ = AboutPanel::view();
    }

    #[test]
    fn about_renders_the_single_source_disclaimer() {
        // E6.9 acceptance #2: the About surface pulls DISCLAIMER.md via
        // mde_disclaimer (the single source), never a copy-paste. Lock
        // that the panel's text IS mde_disclaimer::TEXT and is non-empty.
        assert_eq!(AboutPanel::disclaimer_text(), mde_disclaimer::TEXT);
        assert!(
            !AboutPanel::disclaimer_text().trim().is_empty(),
            "disclaimer text must be embedded + non-empty"
        );
    }
}
