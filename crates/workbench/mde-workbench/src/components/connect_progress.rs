//! MESH-CONNECT-DIALOG-1 — a reusable connect/configure progress modal.
//!
//! Three terminal-bound states reused across the Overview (`home`),
//! Mesh Services, and Music panels so every "Configure" / "Connect" /
//! "Start" button shows real progress and a real outcome — never a
//! silent no-op:
//!
//!   * [`ConnectProgress::Pending`] — a spinner glyph + a "what's
//!     happening" label while a real status action is polled.
//!   * [`ConnectProgress::Success`] — a green check + the outcome line,
//!     dismiss-only.
//!   * [`ConnectProgress::Failure`] — a red error glyph + the error
//!     line, with Retry + Dismiss.
//!
//! The component is pure chrome: it owns no async work and no timers.
//! The host panel drives the state machine (open → poll the relevant
//! `action/<domain>/status` / systemd / daemon probe → set
//! Success/Failure), so the polling stays where the panel's domain
//! knowledge lives. The modal renders over the panel body via
//! [`overlay`], which stacks a click-catching backdrop + the centered
//! dialog (reusing the locked Carbon dialog chrome from
//! [`crate::panel_chrome`]) above the panel's own content.
//!
//! Carbon look (§4): every color is a `mde-theme` palette token — the
//! spinner/title/error use `text` / `success` / `danger`; the dialog
//! shell + backdrop are the shared `panel_chrome` tokens. No raw hex,
//! no scattered metric literals.

use cosmic::iced::widget::{column, container, mouse_area, row, stack, text, Space};
use cosmic::iced::{Element, Length, Padding};

use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// The modal's lifecycle. `Closed` renders nothing (the panel shows its
/// body bare); the other three are the live dialog states.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectProgress {
    /// No modal — the panel body renders bare.
    #[default]
    Closed,
    /// In-flight: a spinner glyph + a label describing the probe
    /// (e.g. "Checking mesh service status…").
    Pending {
        /// Operator-readable title (what's being connected/configured).
        title: String,
        /// One-line "what's happening now" status under the spinner.
        label: String,
    },
    /// Terminal success: a green check + the outcome line. Dismiss-only.
    Success {
        title: String,
        /// The success outcome (e.g. "Connected — 3 of 4 services up").
        message: String,
    },
    /// Terminal failure: a red glyph + the error line. Retry + Dismiss.
    Failure {
        title: String,
        /// The operator-readable error (e.g. "mackesd is not answering").
        error: String,
    },
}

impl ConnectProgress {
    /// Open the modal in its pending state for `title`, with an initial
    /// `label` describing the probe that's about to run.
    #[must_use]
    pub fn pending(title: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Pending {
            title: title.into(),
            label: label.into(),
        }
    }

    /// Resolve the modal to its success state, keeping the open title.
    #[must_use]
    pub fn success(&self, message: impl Into<String>) -> Self {
        Self::Success {
            title: self.title().to_string(),
            message: message.into(),
        }
    }

    /// Resolve the modal to its failure state, keeping the open title.
    #[must_use]
    pub fn failure(&self, error: impl Into<String>) -> Self {
        Self::Failure {
            title: self.title().to_string(),
            error: error.into(),
        }
    }

    /// Is the modal currently shown (any non-`Closed` state)?
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::Closed)
    }

    /// Is the modal still polling (so the host should keep the poll loop
    /// alive)? Only `Pending` is in-flight.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    /// The dialog title for the current state (empty for `Closed`).
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::Closed => "",
            Self::Pending { title, .. }
            | Self::Success { title, .. }
            | Self::Failure { title, .. } => title,
        }
    }
}

/// Stack the modal (when open) over a panel's `body`. Returns `body`
/// unchanged when the modal is `Closed`, so a panel that isn't showing a
/// dialog pays zero extra widgets and keeps its own sizing.
///
/// `on_retry` fires from the Failure state's Retry button; `on_dismiss`
/// fires from the Dismiss button AND from a backdrop click (click-out to
/// close, the Classic ChromeOS dialog contract). Both are only wired in
/// the terminal states — a `Pending` modal has no buttons and its
/// backdrop is inert, so an in-flight probe can't be dismissed out from
/// under itself.
pub fn overlay<'a, Message>(
    state: &ConnectProgress,
    body: Element<'a, Message, cosmic::Theme>,
    palette: Palette,
    on_retry: Message,
    on_dismiss: Message,
) -> Element<'a, Message, cosmic::Theme>
where
    Message: Clone + 'a,
{
    overlay_with_action(state, body, palette, on_retry, on_dismiss, None)
}

/// As [`overlay`], but with an optional terminal-state **primary action**
/// `(label, message)` rendered as an extra primary button (rightmost) in
/// the Success / Failure states — e.g. the Overview's "Open settings ▸",
/// so confirming a capability's status doesn't dead-end (the operator can
/// still reach the panel to configure it). Pass `None` for the plain
/// Retry / Dismiss modal (Mesh Services, Music).
pub fn overlay_with_action<'a, Message>(
    state: &ConnectProgress,
    body: Element<'a, Message, cosmic::Theme>,
    palette: Palette,
    on_retry: Message,
    on_dismiss: Message,
    primary: Option<(&'a str, Message)>,
) -> Element<'a, Message, cosmic::Theme>
where
    Message: Clone + 'a,
{
    if !state.is_open() {
        return body;
    }
    let modal = view(state, palette, on_retry, on_dismiss, primary);
    stack![body, modal].into()
}

/// Render the live modal layer — a backdrop scrim + the centered dialog.
/// Only called for an open `state` (callers go through [`overlay`]).
fn view<'a, Message>(
    state: &ConnectProgress,
    palette: Palette,
    on_retry: Message,
    on_dismiss: Message,
    primary: Option<(&'a str, Message)>,
) -> Element<'a, Message, cosmic::Theme>
where
    Message: Clone + 'a,
{
    let terminal = !state.is_pending();
    // The backdrop intercepts clicks; in a terminal state a backdrop
    // click dismisses (click-out), while a pending probe's backdrop is
    // inert so it can't be dismissed mid-flight.
    let backdrop = crate::panel_chrome::dialog_backdrop::<Message>();
    let backdrop: Element<'a, Message, cosmic::Theme> = if terminal {
        mouse_area(backdrop).on_press(on_dismiss.clone()).into()
    } else {
        backdrop
    };

    let dialog = crate::panel_chrome::dialog(
        dialog_body(state, palette, on_retry, on_dismiss, primary),
        palette,
        mde_theme::Density::Comfortable,
    );

    // Center the dialog within the backdrop's bounds.
    let centered = container(dialog)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center);

    stack![backdrop, centered].into()
}

/// The inner dialog content: title row, the state's icon + body lines,
/// and the action button row (terminal states only).
fn dialog_body<'a, Message>(
    state: &ConnectProgress,
    palette: Palette,
    on_retry: Message,
    on_dismiss: Message,
    primary: Option<(&'a str, Message)>,
) -> Element<'a, Message, cosmic::Theme>
where
    Message: Clone + 'a,
{
    let sizes = FontSize::defaults();
    let title_row = crate::panel_chrome::dialog_title_row::<Message>(state.title(), palette);

    // Icon + status color + body line per state.
    let (icon, icon_color, body_line) = match state {
        ConnectProgress::Closed => (
            Icon::StatusUnknown,
            palette.text_muted.into_cosmic_color(),
            String::new(),
        ),
        ConnectProgress::Pending { label, .. } => (
            // The "pending" status dot stands in for an animated spinner —
            // the workbench has no per-subtree spin transform, so the
            // unknown/pending glyph is the at-rest indeterminate cue, and
            // the live label is what actually advances as the probe runs.
            Icon::StatusUnknown,
            palette.accent.into_cosmic_color(),
            label.clone(),
        ),
        ConnectProgress::Success { message, .. } => (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            message.clone(),
        ),
        ConnectProgress::Failure { error, .. } => (
            Icon::StatusError,
            palette.danger.into_cosmic_color(),
            error.clone(),
        ),
    };

    let status_row = row![
        icon_widget(icon, icon_color),
        Space::new().width(Length::Fixed(10.0)),
        text(body_line)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let status_block = container(status_row).padding(Padding {
        top: 8.0,
        right: 16.0,
        bottom: 8.0,
        left: 16.0,
    });

    let buttons = button_row(state, palette, on_retry, on_dismiss, primary);

    column![title_row, status_block, buttons]
        .spacing(4)
        .width(Length::Shrink)
        .into()
}

/// The action row per state: Pending has none (the probe is running),
/// Success has Dismiss (+ optional primary action), Failure has Retry +
/// Dismiss (+ optional primary action). `dialog_button_row` lays them out
/// right-aligned with the primary rightmost.
fn button_row<'a, Message>(
    state: &ConnectProgress,
    palette: Palette,
    on_retry: Message,
    on_dismiss: Message,
    primary: Option<(&'a str, Message)>,
) -> Element<'a, Message, cosmic::Theme>
where
    Message: Clone + 'a,
{
    use crate::controls::{variant_button, ButtonVariant};
    // The optional caller-supplied primary action (e.g. "Open settings ▸"),
    // rendered rightmost in the terminal states so confirming status doesn't
    // dead-end. When present it takes the Primary slot; Dismiss drops to Ghost.
    let primary_btn = |actions: &mut Vec<Element<'a, Message, cosmic::Theme>>| {
        if let Some((label, msg)) = primary.clone() {
            actions.push(variant_button(
                label,
                ButtonVariant::Primary,
                Some(msg),
                palette,
            ));
        }
    };
    let has_primary = primary.is_some();
    let dismiss_variant = if has_primary {
        ButtonVariant::Ghost
    } else {
        ButtonVariant::Primary
    };
    let actions: Vec<Element<'a, Message, cosmic::Theme>> = match state {
        // No buttons while the probe is in flight — the modal resolves
        // itself when the host's poll lands a terminal outcome.
        ConnectProgress::Closed | ConnectProgress::Pending { .. } => Vec::new(),
        ConnectProgress::Success { .. } => {
            let mut a = vec![variant_button(
                "Dismiss",
                dismiss_variant,
                Some(on_dismiss),
                palette,
            )];
            primary_btn(&mut a);
            a
        }
        ConnectProgress::Failure { .. } => {
            let mut a = vec![
                variant_button("Dismiss", ButtonVariant::Ghost, Some(on_dismiss), palette),
                variant_button(
                    "Retry",
                    if has_primary {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Primary
                    },
                    Some(on_retry),
                    palette,
                ),
            ];
            primary_btn(&mut a);
            a
        }
    };
    crate::panel_chrome::dialog_button_row(actions)
}

/// Render an `mde-theme` status icon at inline size in `color`, falling
/// back to its glyph when the SVG asset is absent.
fn icon_widget<'a, Message: 'a>(
    icon: Icon,
    color: cosmic::iced::Color,
) -> Element<'a, Message, cosmic::Theme> {
    let resolved = mde_icon(icon, IconSize::PanelHeader);
    if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .sty(move |_t: &cosmic::Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .colr(color)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_is_not_open_and_overlay_passes_body_through() {
        let s = ConnectProgress::Closed;
        assert!(!s.is_open());
        assert!(!s.is_pending());
        // overlay returns the bare body (no panic, no extra layer).
        let body: Element<'_, (), cosmic::Theme> = cosmic::iced::widget::text("body").into();
        let _ = overlay(&s, body, crate::live_theme::palette(), (), ());
    }

    #[test]
    fn pending_is_open_and_pending() {
        let s = ConnectProgress::pending("Connect", "Checking…");
        assert!(s.is_open());
        assert!(s.is_pending());
        assert_eq!(s.title(), "Connect");
    }

    #[test]
    fn success_keeps_title_and_is_terminal() {
        let s = ConnectProgress::pending("Connect music", "Pinging…")
            .success("Connected (API v1.16.1).");
        assert!(s.is_open());
        assert!(!s.is_pending());
        assert_eq!(s.title(), "Connect music");
        assert!(matches!(s, ConnectProgress::Success { .. }));
    }

    #[test]
    fn failure_keeps_title_and_is_terminal() {
        let s =
            ConnectProgress::pending("Start services", "Probing…").failure("mackesd not answering");
        assert!(s.is_open());
        assert!(!s.is_pending());
        assert_eq!(s.title(), "Start services");
        assert!(matches!(s, ConnectProgress::Failure { .. }));
    }

    #[test]
    fn view_renders_each_open_state_without_panic() {
        let palette = crate::live_theme::palette();
        for s in [
            ConnectProgress::pending("t", "l"),
            ConnectProgress::pending("t", "l").success("ok"),
            ConnectProgress::pending("t", "l").failure("err"),
        ] {
            // Both the plain (no primary action) and the primary-action forms.
            let _: Element<'_, (), cosmic::Theme> = view(&s, palette, (), (), None);
            let _: Element<'_, (), cosmic::Theme> =
                view(&s, palette, (), (), Some(("Open settings", ())));
        }
    }

    #[test]
    fn overlay_with_action_passes_body_through_when_closed() {
        let body: Element<'_, (), cosmic::Theme> = cosmic::iced::widget::text("body").into();
        let _ = overlay_with_action(
            &ConnectProgress::Closed,
            body,
            crate::live_theme::palette(),
            (),
            (),
            Some(("Open settings", ())),
        );
    }
}
