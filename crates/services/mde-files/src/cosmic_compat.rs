//! GUI-7 — thin per-widget `.sty()` shims bridging the iced-style per-widget
//! style closures the Artifact Manager was written against to libcosmic's
//! class-based theming.
//!
//! libcosmic renders with `cosmic::Theme`, whose catalog classes for `button`
//! and `svg` do **not** implement `From<StyleFn>` (so the iced `.style(closure)`
//! convenience doesn't type-check), while `container` does. Rather than rewrite
//! ~60 closures into `.class(cosmic::theme::*::custom(..))` by hand, these
//! extension traits expose a uniform `.sty(closure)` that wraps the closure in
//! the right cosmic class. Method resolution picks the impl by receiver type,
//! so a single mechanical `.style(` → `.sty(` rename ports every call site.

use cosmic::iced::widget::{button, container, svg};
use cosmic::iced::widget::{Button, Container, Svg, Text};
use cosmic::iced::Color;
use cosmic::Theme;

/// `.sty(|theme| container::Style { .. })` → cosmic container class.
pub trait ContainerSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self;
}

impl<'a, M: 'a> ContainerSty<'a, M> for Container<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self {
        // The iced-catalog container Class *does* impl `From<StyleFn>`, so the
        // native `.style()` already works for cosmic::Theme — delegate to it.
        self.style(f)
    }
}

/// `.sty(|theme, status| button::Style { .. })` → cosmic status-aware button
/// class (`cosmic::theme::Button::Custom`).
pub trait ButtonSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self;
}

impl<'a, M: 'a> ButtonSty<'a, M> for Button<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self {
        self.class(cosmic::theme::iced::Button::Custom(Box::new(f)))
    }
}

/// `.sty(|theme| svg::Style { .. })` → cosmic svg class. (cosmic's svg custom
/// is statusless, so the source closures drop their `svg::Status` arg.)
pub trait SvgSty<'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme) -> svg::Style + 'static) -> Self;
}

impl<'a> SvgSty<'a> for Svg<'a, Theme> {
    fn sty(self, f: impl Fn(&Theme) -> svg::Style + 'static) -> Self {
        self.class(cosmic::theme::iced::Svg::custom(f))
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
