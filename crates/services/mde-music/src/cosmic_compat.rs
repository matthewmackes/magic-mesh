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

use cosmic::iced::widget::{button, Button, Text};
use cosmic::iced::Color;
use cosmic::Theme;

/// MUSIC-ALBUMS — `.sty(|theme, status| button::Style { .. })` → cosmic
/// status-aware button class. Ported from `mde-files` so the Carbon sidebar /
/// nav rows can render flat (transparent idle, raised/active) instead of the
/// default themed button chrome.
pub trait ButtonSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self;
}

impl<'a, M: 'a> ButtonSty<'a, M> for Button<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self {
        self.class(cosmic::theme::iced::Button::Custom(Box::new(f)))
    }
}

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
