//! UX-6 empty-state component — data form.
//!
//! Every panel needs a polished zero-data view. Before UX-6
//! each panel rolled its own ad-hoc placeholder text; this
//! component standardizes the data shape:
//!
//!   * an icon (UX-8 swap-in target — 32 px slot reserved)
//!   * a heading (TypeRole::Heading)
//!   * a body line (TypeRole::Body, text_muted)
//!   * an optional CTA button (primary fill)
//!
//! The data struct lives here so non-Iced consumers can
//! describe a panel without pulling in the Iced runtime. The
//! widget builder lives in
//! `crates/mde-workbench/src/panel_chrome.rs` so the toolkit
//! dep (iced) doesn't leak into `mde-theme`.

use crate::color::Rgba;
use crate::icons::Icon;

/// Describes the contents of a panel's zero-data view. Carries
/// the strings + the optional CTA label; the renderer supplies
/// the actual visuals.
#[derive(Debug, Clone, PartialEq)]
pub struct EmptyState {
    /// Optional hero icon (UX-8). When `None`, the renderer
    /// reserves the icon slot as empty space.
    pub icon: Option<Icon>,
    /// One-line heading. Conveys what's missing (e.g.,
    /// "No snapshots yet").
    pub heading: String,
    /// One-or-two-line body. Tells the user how to get started.
    pub body: String,
    /// Optional CTA label. When `Some`, the renderer paints a
    /// primary-fill button beneath the body.
    pub cta_label: Option<String>,
    /// Reserved colour override for the body text. `None` =
    /// `palette.text_muted`. UX-6 keeps this read-only for
    /// future a11y variants (UX-22 high-contrast may force
    /// a different muted token).
    pub body_color_override: Option<Rgba>,
}

impl EmptyState {
    /// Build a basic empty-state with no CTA.
    #[must_use]
    pub fn info(heading: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            icon: None,
            heading: heading.into(),
            body: body.into(),
            cta_label: None,
            body_color_override: None,
        }
    }

    /// Build an empty-state with a CTA button. The renderer
    /// pairs the returned struct with a click handler supplied
    /// by the call site.
    #[must_use]
    pub fn with_cta(
        heading: impl Into<String>,
        body: impl Into<String>,
        cta_label: impl Into<String>,
    ) -> Self {
        Self {
            icon: None,
            heading: heading.into(),
            body: body.into(),
            cta_label: Some(cta_label.into()),
            body_color_override: None,
        }
    }

    /// Builder: attach a hero icon (UX-8). Use in conjunction
    /// with `info` / `with_cta`:
    /// `EmptyState::with_cta(...).with_icon(Icon::Fleet)`.
    #[must_use]
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// UX-6 — icon slot reserved at 32 px so the post-UX-8 swap is
/// one-line. Component dimension, not density-scaled.
pub const EMPTY_ICON_SIZE: f32 = 32.0;

/// Gap between heading and body in the rendered empty-state.
/// Matches `Space::sm` (8 px) at comfortable density.
pub const HEADING_BODY_GAP: f32 = 8.0;

/// Gap between body and CTA. Slightly larger so the CTA reads
/// as a distinct affordance, not part of the prose.
pub const BODY_CTA_GAP: f32 = 16.0;

/// Vertical padding above + below the empty-state content,
/// applied by the renderer so the block sits visually centered
/// inside the panel body.
pub const VERTICAL_PADDING: f32 = 48.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_constructor_has_no_cta() {
        let e = EmptyState::info("Heading", "Body");
        assert_eq!(e.cta_label, None);
        assert_eq!(e.heading, "Heading");
        assert_eq!(e.body, "Body");
    }

    #[test]
    fn with_cta_constructor_carries_label() {
        let e = EmptyState::with_cta("Heading", "Body", "Get started");
        assert_eq!(e.cta_label.as_deref(), Some("Get started"));
    }

    #[test]
    fn icon_slot_is_thirty_two_px() {
        // UX-6 spec — 32 px icon tier for empty-state.
        assert!((EMPTY_ICON_SIZE - 32.0).abs() < f32::EPSILON);
    }

    #[test]
    fn body_color_override_starts_none() {
        let e = EmptyState::info("h", "b");
        assert_eq!(e.body_color_override, None);
    }

    #[test]
    fn with_icon_builder_attaches_icon() {
        let e = EmptyState::info("h", "b").with_icon(Icon::Fleet);
        assert_eq!(e.icon, Some(Icon::Fleet));
    }
}
