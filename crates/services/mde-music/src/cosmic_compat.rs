//! EFF-34 — thin per-widget `.colr()` shim bridging the iced-style per-widget
//! text-colour closures mde-music was written against to libcosmic's
//! class-based theming.
//!
//! libcosmic renders with `cosmic::Theme`. The fork's `text().color()` builds a
//! `StyleFn`, but `cosmic::theme::iced::Text` only impls `From<Color>` (not
//! `From<StyleFn>`), so the iced `.color(closure-ish)` convenience doesn't
//! type-check; this extension trait sets the colour through the class directly.
//! A single mechanical `.color(` → `.colr(` rename ports every call site.
//!
//! (Distilled from `mde-files/src/cosmic_compat.rs` — the GUI-7 reference recipe
//! — keeping only the `Text` shim, since mde-music's container styles use the
//! native `.style()` and it has no per-widget button/svg style closures.)

use cosmic::iced::widget::Text;
use cosmic::iced::Color;
use cosmic::Theme;

/// `.colr(color)` → cosmic text color class. The fork's `text().color()`
/// builds a `StyleFn`, but `cosmic::theme::iced::Text` only impls `From<Color>`
/// (not `From<StyleFn>`), so set the color through the class directly.
pub trait TextSty<'a> {
    #[must_use]
    fn colr(self, color: impl Into<Color>) -> Self;
}

impl<'a> TextSty<'a> for Text<'a, Theme> {
    fn colr(self, color: impl Into<Color>) -> Self {
        self.class(cosmic::theme::iced::Text::Color(color.into()))
    }
}
