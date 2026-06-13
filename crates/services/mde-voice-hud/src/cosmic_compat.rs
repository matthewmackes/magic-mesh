//! CUT-2 — libcosmic compat shims for the voice-HUD port. Bridges the
//! iced-style per-widget `.style(closure)` the HUD was written against to
//! libcosmic's class-based theming, mirroring mde-files'/the workbench's
//! cosmic_compat. A mechanical `.style(` → `.sty(` rename ports every site;
//! method resolution picks the impl by receiver type.

use cosmic::iced::widget::{button, container};
use cosmic::iced::widget::{Button, Container, Text};
use cosmic::iced::Color;
use cosmic::Theme;

/// `.sty(|theme| container::Style { .. })` → cosmic container class.
pub trait ContainerSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self;
}
impl<'a, M: 'a> ContainerSty<'a, M> for Container<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self {
        self.style(f)
    }
}

/// `.sty(|theme, status| button::Style { .. })` → cosmic status-aware button class.
pub trait ButtonSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self;
}
impl<'a, M: 'a> ButtonSty<'a, M> for Button<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self {
        self.class(cosmic::theme::iced::Button::Custom(Box::new(f)))
    }
}

/// `.colr(color)` → cosmic text color class.
pub trait TextSty<'a> {
    #[must_use]
    fn colr(self, color: impl Into<Color>) -> Self;
}
impl<'a> TextSty<'a> for Text<'a, Theme> {
    fn colr(self, color: impl Into<Color>) -> Self {
        self.class(cosmic::theme::iced::Text::Color(color.into()))
    }
}

pub mod prelude {
    pub use super::{ButtonSty, ContainerSty, TextSty};
}
