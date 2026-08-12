//! Shared egui widgets built on the single-source [`crate::Style`].
//!
//! The E12 equivalent of the retired `mde-theme::components`: surfaces reuse
//! these instead of re-typing the same idiom, so a look lives in ONE place
//! (§6 glue; `/polish` axis 7 — component reuse & consolidation).

use egui::{Color32, CornerRadius, Frame, InnerResponse, Margin, Response, Sense, Stroke, Ui};

use crate::{
    carbon::paint_carbon,
    style::{Elevation, TypographyRole},
    Style,
};

// ── Surface primitives (UI-VIS-111) ─────────────────────────────────────────
// The shared replacements for a hand-rolled `egui::Frame::group` / bordered
// rectangle: one card / inset / toolbar / section / dialog / overlay each, with
// controlled fill, stroke, margin, rounding, and (where it lifts) a soft shadow
// from the shared `Style` tokens and the `Elevation` ladder. Every one returns a
// configured `egui::Frame`, so a surface shows content the idiomatic way —
// `card().show(ui, |ui| { … })` — and can still override a field when it must.
// Hierarchy comes from these plus typography/spacing, NOT from boxing every
// region (a `section` is deliberately frameless).

/// Convert a corner-radius token (logical px — [`Style::RADIUS_S`] …) into an
/// [`egui::CornerRadius`]. The one place the `f32` radius tiers become egui's
/// integer corner type, so a surface never re-casts a rounding value.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn corner(radius: f32) -> CornerRadius {
    CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8)
}

/// A **card** — a persistent raised surface for grouped content: the base surface
/// fill, a hairline border, the mid corner radius, comfortable inner padding, and
/// a soft raised shadow. The shared replacement for `egui::Frame::group` plus a
/// per-surface `card_shadow()`. Show content with `card().show(ui, |ui| { … })`.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn card() -> Frame {
    Frame::NONE
        .fill(Style::SURFACE)
        .stroke(Style::hairline())
        .corner_radius(corner(Style::RADIUS_M))
        .inner_margin(Margin::same(Style::SP_M as i8))
        .shadow(Elevation::Raised.egui_shadow())
}

/// An **inset** — a recessed well for an input or a nested read-only region: the
/// deep app-background fill, a hairline border, the tight corner radius, snug
/// padding, and no shadow (it sits *into* its parent, not above it).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn inset() -> Frame {
    Frame::NONE
        .fill(Style::BG)
        .stroke(Style::hairline())
        .corner_radius(corner(Style::RADIUS_S))
        .inner_margin(Margin::same(Style::SP_S as i8))
}

/// A **toolbar** — a quiet chrome strip around controls: the base surface fill,
/// no border, the tight corner radius, and the shared refined toolbar margin.
/// Deliberately flat (no shadow) so inactive chrome stays visually quiet
/// (UI-VIS-109).
#[must_use]
pub fn toolbar() -> Frame {
    Frame::NONE
        .fill(Style::SURFACE)
        .corner_radius(corner(Style::RADIUS_S))
        .inner_margin(Style::toolbar_margin())
}

/// A **section** — a typographic grouping with breathing room but *no* box: no
/// fill, no stroke, only vertical rhythm. Prefer this over a bordered frame when
/// sectioning (UI-VIS-114 — hierarchy from spacing and typography, not
/// containers).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn section() -> Frame {
    Frame::NONE.inner_margin(Margin::symmetric(0, Style::SP_S as i8))
}

/// A **dialog** — a modal sheet: the base surface fill, a hairline border, the
/// large corner radius, generous padding, and the deep modal shadow (UI-VIS-111).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn dialog() -> Frame {
    Frame::NONE
        .fill(Style::SURFACE)
        .stroke(Style::hairline())
        .corner_radius(corner(Style::RADIUS_L))
        .inner_margin(Margin::same(Style::SP_L as i8))
        .shadow(Elevation::Modal.egui_shadow())
}

/// An **overlay** — a floating menu / popover / tooltip surface: the base fill, a
/// hairline border, the mid corner radius, compact padding, and the overlay
/// shadow (UI-VIS-111/125).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn overlay() -> Frame {
    Frame::NONE
        .fill(Style::SURFACE)
        .stroke(Style::hairline())
        .corner_radius(corner(Style::RADIUS_M))
        .inner_margin(Margin::same(Style::SP_S as i8))
        .shadow(Elevation::Overlay.egui_shadow())
}

/// Paint a shared tooltip surface using the current Quazar appearance.
///
/// Tooltip content is deliberately rendered through the shared overlay frame
/// rather than egui's default `on_hover_text` popup, so Dark/Light surfaces,
/// typography, padding, border, and expressive motion stay centralized.
pub fn tooltip(ui: &mut Ui, text: &str) {
    let surface = Style::resolve_color(ui.ctx(), Style::SURFACE);
    let border = Style::resolve_color(ui.ctx(), Style::BORDER);
    overlay()
        .fill(surface)
        .stroke(Stroke::new(Style::STROKE_HAIRLINE, border))
        .show(ui, |ui| {
            let text_color = Style::resolve_color(ui.ctx(), Style::TEXT);
            ui.label(Style::typography_text(text, TypographyRole::Caption).color(text_color));
        });
}

/// Attach a shared tooltip to a response.
pub fn hover_text(response: Response, text: impl Into<String>) -> Response {
    let text = text.into();
    response.on_hover_ui(move |ui| tooltip(ui, text.as_str()))
}

/// Attach a shared tooltip to a disabled response.
pub fn disabled_hover_text(response: Response, text: impl Into<String>) -> Response {
    let text = text.into();
    response.on_disabled_hover_ui(move |ui| tooltip(ui, text.as_str()))
}

/// A **muted note** — a dim, small-caption label — returning its [`Response`].
///
/// This is the single source for the "honestly empty / not-yet-reported" panel
/// state (and any small secondary caption): [`Style::TEXT_DIM`] at the shared
/// `Caption` type size. It consolidates the
/// `ui.colored_label(Style::TEXT_DIM, Style::typography_text(msg, Caption))`
/// idiom that was hand-rolled across every surface — restyle the empty/caption
/// look in one place now, and every panel follows.
///
/// Use it for a genuinely-empty state (no data yet), NOT as a stand-in for real
/// content (§7 — a muted note is an honest "nothing here", never a mockup).
pub fn muted_note(ui: &mut Ui, msg: impl Into<String>) -> Response {
    ui.colored_label(
        Style::TEXT_DIM,
        Style::typography_text(msg, TypographyRole::Caption),
    )
}

/// The height of one dense operational list or table row for `density`.
///
/// Dense does not mean undersized: pointer surfaces keep the compact control
/// rung, while touch and large-text configurations grow to their minimum hit
/// target. Tables and palettes therefore remain scannable without creating
/// separate per-workspace row metrics.
#[must_use]
pub const fn dense_row_height(density: crate::Density) -> f32 {
    Style::CONTROL_H_M.max(density.min_hit_target())
}

/// Render one zebra-striped row in a dense operational list or table.
///
/// `index` is the stable visual row index (including rows filtered from a
/// palette), so alternating treatment stays deterministic across repaint. The
/// closure owns the row's real controls and routing; this helper owns only the
/// shared density, surface treatment, and horizontal rhythm.
pub fn striped_row<R>(
    ui: &mut Ui,
    index: usize,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let fill = if index % 2 == 0 {
        Style::resolve_color(ui.ctx(), Style::SURFACE)
    } else {
        Style::resolve_color(ui.ctx(), Style::SURFACE_HI).gamma_multiply(0.45)
    };
    Frame::NONE
        .fill(fill)
        .inner_margin(Margin::symmetric(Style::SP_S as i8, Style::SP_XS as i8))
        .show(ui, |ui| {
            ui.set_min_height(dense_row_height(Style::density(ui.ctx())));
            add_contents(ui)
        })
}

/// The shared dense data-presentation scaffold. It carries a deterministic
/// zebra index for tables, palettes, and operational lists while callers retain
/// their own columns, selection, and data models.
#[derive(Debug, Default)]
pub struct DenseList {
    next_row: usize,
}

impl DenseList {
    /// Start an empty dense list.
    #[must_use]
    pub const fn new() -> Self {
        Self { next_row: 0 }
    }

    /// Render a caller-owned header above the striped rows.
    pub fn header<R>(&mut self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        let response = toolbar().show(ui, |ui| {
            ui.set_min_height(dense_row_height(Style::density(ui.ctx())));
            add_contents(ui)
        });
        response.inner
    }

    /// Render the next deterministic striped row.
    pub fn row<R>(
        &mut self,
        ui: &mut Ui,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<R> {
        let response = striped_row(ui, self.next_row, add_contents);
        self.next_row = self.next_row.saturating_add(1);
        response
    }
}

/// The semantic condition a workspace-state panel communicates. State is not
/// inferred from colour alone: each variant supplies explicit copy, while the
/// caller retains its real retry, reconnect, or confirmation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceState {
    /// Work is in progress and no stable result is available yet.
    Loading,
    /// The source answered successfully but contains no items.
    Empty,
    /// The view is usable but backed by an older snapshot.
    Stale,
    /// The live source cannot currently be reached.
    Offline,
    /// The requested operation failed.
    Error,
    /// An irreversible or high-impact action needs deliberate confirmation.
    Destructive,
}

impl WorkspaceState {
    /// Canonical operator-facing language for this condition.
    ///
    /// This label is owned by the shared component rather than its caller so a
    /// stale or confused surface cannot make (for example) an offline panel
    /// read as healthy to either a sighted operator or assistive technology.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loading => "Loading",
            Self::Empty => "Empty",
            Self::Stale => "Stale",
            Self::Offline => "Offline",
            Self::Error => "Error",
            Self::Destructive => "Confirmation required",
        }
    }

    const fn icon(self) -> &'static str {
        match self {
            Self::Loading => "process-stop",
            Self::Empty => "text-x-generic",
            Self::Stale => "view-refresh",
            Self::Offline => "changes-prevent",
            Self::Error | Self::Destructive => "dialog-warning",
        }
    }

    const fn tone(self) -> Color32 {
        match self {
            Self::Loading | Self::Empty | Self::Stale => Style::TEXT_DIM,
            Self::Offline | Self::Error | Self::Destructive => Style::DANGER,
        }
    }
}

/// A centered, semantic workspace state with optional caller-owned actions.
///
/// The shared kit owns visual hierarchy, icon treatment, and density-aware
/// spacing; each workspace keeps its behavioral contract and action routing.
pub struct WorkspaceStatePanel<'a> {
    state: WorkspaceState,
    title: &'a str,
    detail: &'a str,
}

impl<'a> WorkspaceStatePanel<'a> {
    /// Create a state panel with explicit operator-facing copy.
    #[must_use]
    pub const fn new(state: WorkspaceState, title: &'a str, detail: &'a str) -> Self {
        Self {
            state,
            title,
            detail,
        }
    }

    /// Render the panel and the caller-owned action area.
    pub fn show<R>(self, ui: &mut Ui, add_actions: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        // `card()` adds SP_M on both sides.  Budget those insets before sizing
        // the content lane; otherwise the shared empty/offline/error surface is
        // wider than its viewport on narrow and largest-text layouts.
        let outer_width = ui.available_width().min(Style::SP_XL * 16.0);
        let content_width = 2.0f32
            .mul_add(
                -Style::STROKE_HAIRLINE,
                2.0f32.mul_add(-Style::SP_M, outer_width),
            )
            .max(1.0);
        let surface = Style::resolve_color(ui.ctx(), Style::SURFACE);
        let border = Style::resolve_color(ui.ctx(), Style::BORDER);
        let title_tone = Style::resolve_color(ui.ctx(), Style::TEXT_STRONG);
        let detail_tone = Style::resolve_color(ui.ctx(), Style::TEXT_DIM);
        let state_tone = Style::resolve_color(ui.ctx(), self.state.tone());
        let state = self.state;
        let title = self.title;
        let detail = self.detail;
        let response = ui.vertical_centered(|ui| {
            ui.add_space(Style::SP_XL);
            card()
                .fill(surface)
                .stroke(Stroke::new(Style::STROKE_HAIRLINE, border))
                .show(ui, |ui| {
                    ui.set_width(content_width);
                    ui.vertical_centered(|ui| {
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(Style::ICON_XL, Style::ICON_XL),
                            Sense::hover(),
                        );
                        let _ = paint_carbon(ui.painter(), icon_rect, state.icon(), state_tone);
                        ui.add_space(Style::SP_S);
                        ui.label(
                            Style::typography_text(
                                state.label().to_uppercase(),
                                TypographyRole::Caption,
                            )
                            .color(state_tone),
                        );
                        ui.add_space(Style::SP_XS);
                        ui.add(
                            egui::Label::new(
                                Style::typography_text(title, TypographyRole::Headline)
                                    .color(title_tone),
                            )
                            .wrap(),
                        );
                        ui.add_space(Style::SP_XS);
                        ui.add(
                            egui::Label::new(
                                Style::typography_text(detail, TypographyRole::Body)
                                    .color(detail_tone),
                            )
                            .wrap(),
                        );
                        ui.add_space(Style::SP_M);
                        add_actions(ui)
                    })
                    .inner
                })
                .inner
        });
        response.response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!("{}: {title}. {detail}", state.label()),
            )
        });
        response
    }
}

/// A small filled **status dot** — a [`Style::SP_S`]-sized circle in `color` —
/// for an inline health/presence indicator beside a label.
///
/// The single source for the primitive that mde-files-egui and mde-voice-egui
/// hand-rolled byte-identically. `color` is a `Style` palette token
/// ([`Style::OK`]/[`Style::WARN`]/[`Style::DANGER`]/…), never a raw literal.
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let diameter = Style::SP_S;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(diameter, diameter), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter * 0.28, color);
}

/// A **labelled value row** on the spacing grid — a dim shared `Caption` `label`,
/// a [`Style::SP_S`] gutter, then a `tone`-coloured `value` at the same size.
///
/// The single source for the "field" row that several shell panels hand-rolled
/// byte-identically. `tone` is a `Style` palette token (never a raw literal).
pub fn field(ui: &mut Ui, label: &str, value: &str, tone: Color32) {
    ui.horizontal(|ui| {
        ui.label(Style::typography_text(label, TypographyRole::Caption).color(Style::TEXT_DIM));
        ui.add_space(Style::SP_S);
        ui.colored_label(tone, Style::typography_text(value, TypographyRole::Caption));
    });
}

/// Compact display data for a shell-wide operation progress badge.
///
/// This intentionally contains only the bounded UI projection: detailed file,
/// transfer, or storage job models stay with their owning surface/worker. Shared
/// chrome can use this primitive to render one consistent progress affordance
/// without depending on those models.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OperationProgressView<'a> {
    /// Active jobs represented by this summary.
    pub active: usize,
    /// Average known progress, `None` while all active jobs are still queued.
    pub fraction: Option<f32>,
    /// Already-bounded display label.
    pub label: &'a str,
}

impl<'a> OperationProgressView<'a> {
    /// Construct a compact operation-progress view.
    #[must_use]
    pub fn new(active: usize, fraction: Option<f32>, label: &'a str) -> Self {
        Self {
            active,
            fraction: fraction.map(|f| f.clamp(0.0, 1.0)),
            label,
        }
    }
}

/// Human-readable text painted inside a compact operation progress badge.
#[must_use]
pub fn operation_progress_text(progress: OperationProgressView<'_>) -> String {
    match progress.fraction {
        Some(fraction) => format!("{} - {:.0}%", progress.label, fraction * 100.0),
        None => format!("{} - starting", progress.label),
    }
}

fn operation_progress_status_text(progress: OperationProgressView<'_>) -> String {
    match progress.fraction {
        Some(fraction) => format!("{:.0}%", fraction * 100.0),
        None => "starting".to_string(),
    }
}

fn active_file_operations_text(active: usize) -> String {
    match active {
        1 => "1 active file operation".to_string(),
        active => format!("{active} active file operations"),
    }
}

/// AccessKit value text for a compact operation progress badge.
#[must_use]
pub fn operation_progress_value(progress: OperationProgressView<'_>) -> String {
    let active = active_file_operations_text(progress.active);
    match progress.fraction {
        Some(fraction) => format!("{}, {:.0}% average progress", active, fraction * 100.0),
        None => format!("{active}, progress pending"),
    }
}

/// Paint a compact operation progress badge into an already-allocated rect.
///
/// Callers keep ownership of routing, interaction, and accessibility IDs; this
/// primitive owns the visual language so file/transfer/storage progress does not
/// fork into separate ad-hoc bars.
pub fn paint_operation_progress_badge(
    ui: &Ui,
    rect: egui::Rect,
    progress: OperationProgressView<'_>,
    selected: bool,
    hovered: bool,
) {
    let fill = if selected {
        Style::selection_wash()
    } else if hovered {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    ui.painter().rect_filled(rect, Style::RADIUS, fill);
    ui.painter().rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    if selected {
        let underline_h = Style::SP_XS;
        let underline = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - underline_h),
            rect.right_bottom(),
        );
        ui.painter()
            .rect_filled(underline, egui::CornerRadius::ZERO, Style::ACCENT);
    }

    let clip = rect.shrink(Style::SP_XS);
    let painter = ui.painter().with_clip_rect(clip);
    let font = Style::typography_font(TypographyRole::Caption);
    let status = operation_progress_status_text(progress);
    let status_color = if progress.fraction.is_some() {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    };
    let status_width = painter
        .layout_no_wrap(status.clone(), font.clone(), status_color)
        .size()
        .x;
    let text_y = rect.top() + Style::SP_XS;
    let status_left = rect.right() - Style::SP_S - status_width;
    let label_max_x = (status_left - Style::SP_XS).max(rect.left() + Style::SP_S);
    let label_clip = egui::Rect::from_min_max(
        egui::pos2(rect.left() + Style::SP_S, rect.top()),
        egui::pos2(label_max_x, rect.bottom()),
    );
    if label_clip.width() >= Style::SP_M {
        painter.with_clip_rect(label_clip).text(
            egui::pos2(rect.left() + Style::SP_S, text_y),
            egui::Align2::LEFT_TOP,
            progress.label,
            font.clone(),
            Style::TEXT,
        );
    }
    painter.text(
        egui::pos2(rect.right() - Style::SP_S, text_y),
        egui::Align2::RIGHT_TOP,
        status,
        font,
        status_color,
    );

    let bar = egui::Rect::from_min_size(
        egui::pos2(rect.left() + Style::SP_S, rect.bottom() - Style::SP_S - 5.0),
        egui::vec2((rect.width() - Style::SP_M).max(Style::SP_S), 5.0),
    );
    painter.rect_filled(bar, Style::RADIUS_S, Style::LAYER_01);
    painter.rect_stroke(
        bar,
        Style::RADIUS_S,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    // A queued operation has no measured progress yet. Keep the track visible,
    // but do not paint a guessed fill: any non-zero fill would make the pending
    // state look as though work had already completed in part.
    if let Some(fraction) = progress.fraction {
        let fill_w = bar.width() * fraction.clamp(0.0, 1.0);
        if fill_w > 0.0 {
            let filled = egui::Rect::from_min_size(bar.min, egui::vec2(fill_w, bar.height()));
            painter.rect_filled(filled, Style::RADIUS_S, Style::ACCENT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_note_renders_a_real_small_dim_label() {
        // Headless render: the widget must lay out a real caption (non-zero
        // height, no panic) — proving it's a live render, not dead code — and it
        // reads the shared Style rather than a hand-rolled colour/size.
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = muted_note(ui, "not yet reported");
                assert!(r.rect.height() > 0.0, "muted_note laid out nothing");
                // Accepts both &str and String callers (the two call-site shapes).
                let owned = String::from("roster not yet reported");
                let _ = muted_note(ui, owned);
            });
        });
    }

    #[test]
    fn status_dot_paints_without_panicking() {
        // Headless render: allocating + painting the dot must not panic, and it
        // takes a Style palette token (proving the primitive is live, reachable
        // code both surfaces can share).
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                status_dot(ui, Style::OK);
                status_dot(ui, Style::DANGER);
                // The labelled-value row lays out (dim label + toned value).
                field(ui, "role", "lighthouse", Style::OK);
            });
        });
    }

    #[test]
    fn workspace_states_render_all_honest_conditions() {
        let ctx = egui::Context::default();
        Style::install_with_density(&ctx, crate::Density::Touch);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for state in [
                    WorkspaceState::Loading,
                    WorkspaceState::Empty,
                    WorkspaceState::Stale,
                    WorkspaceState::Offline,
                    WorkspaceState::Error,
                    WorkspaceState::Destructive,
                ] {
                    WorkspaceStatePanel::new(state, "Status", "An honest workspace condition")
                        .show(ui, |ui| ui.button("Retry"));
                }
            });
        });
        assert!(
            !out.shapes.is_empty(),
            "state panels must paint in a headless frame"
        );
    }

    #[test]
    fn workspace_state_panel_stays_bounded_and_uses_light_palette_when_narrow() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(
            &ctx,
            crate::StyleColorScheme::Light,
            crate::Density::Touch,
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(112.0, 480.0));
        let mut panel_rect = egui::Rect::NOTHING;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let available = ui.max_rect();
                    let response = WorkspaceStatePanel::new(
                        WorkspaceState::Offline,
                        "Connection unavailable",
                        "The source is offline; reconnect when it becomes available.",
                    )
                    .show(ui, |ui| ui.button("Reconnect"));
                    panel_rect = response.response.rect;
                    assert!(
                        panel_rect.left() >= available.left()
                            && panel_rect.right() <= available.right(),
                        "shared state panel escaped its narrow viewport: panel={panel_rect:?}, available={available:?}"
                    );
                });
            },
        );

        assert!(
            panel_rect.is_positive(),
            "state panel must allocate real geometry"
        );
        assert!(
            out.shapes.iter().any(|clipped| {
                matches!(
                    &clipped.shape,
                    egui::Shape::Rect(rect)
                        if rect.fill == Style::QUAZAR_LIGHT_SURFACE
                )
            }),
            "windowed Light mode must paint the shared Quazar Light card before any DRM remap"
        );
        let text = painted_text(&out.shapes);
        assert!(
            text.iter().any(|(value, color)| {
                value == "Connection unavailable" && *color == Style::QUAZAR_LIGHT_TEXT_STRONG
            }),
            "state title must use the installed Light palette: {text:?}"
        );
        assert!(
            text.iter().any(|(value, color)| {
                value == "The source is offline; reconnect when it becomes available."
                    && *color == Style::QUAZAR_LIGHT_TEXT_DIM
            }),
            "state detail must use the installed Light palette: {text:?}"
        );
    }

    #[test]
    fn hostile_caller_copy_cannot_erase_canonical_workspace_state_language() {
        let state = WorkspaceState::Offline;
        assert_eq!(state.label(), "Offline");

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                WorkspaceStatePanel::new(
                    state,
                    "Connected and healthy",
                    "All live services are available.",
                )
                .show(ui, |_| ());
            });
        });
        let text = painted_text(&out.shapes);

        assert!(
            text.iter()
                .any(|(value, color)| { value == "OFFLINE" && *color == Style::DANGER }),
            "caller-controlled copy must not erase the shared visible state: {text:?}"
        );
    }

    #[test]
    fn dense_list_keeps_zebra_rows_usable_at_desktop_narrow_and_large_text_sizes() {
        for (size, density) in [
            (egui::vec2(1280.0, 800.0), crate::Density::Mouse),
            (egui::vec2(480.0, 800.0), crate::Density::Mouse),
            (egui::vec2(800.0, 600.0), crate::Density::Touch),
        ] {
            let ctx = egui::Context::default();
            Style::install_with_density(&ctx, density);
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut list = DenseList::new();
                        list.header(ui, |ui| ui.label("Command"));
                        for label in ["New window", "Split pane", "Detach"] {
                            let response = list.row(ui, |ui| ui.selectable_label(false, label));
                            assert!(
                                response.response.rect.height() >= dense_row_height(density),
                                "dense row must keep its target at {size:?}"
                            );
                        }
                    });
                },
            );
            assert!(!out.shapes.is_empty(), "dense list must render at {size:?}");
        }
    }

    #[test]
    fn operation_progress_badge_uses_shared_text_value_and_paints_shapes() {
        let progress = OperationProgressView::new(2, Some(0.42), "2 browser downloads");
        let progress_text = operation_progress_text(progress);
        assert_eq!(progress_text, "2 browser downloads - 42%");
        assert!(progress_text.is_ascii());
        assert_eq!(
            operation_progress_value(progress),
            "2 active file operations, 42% average progress"
        );
        let pending = OperationProgressView::new(1, None, "Copy report.txt");
        let pending_text = operation_progress_text(pending);
        assert_eq!(pending_text, "Copy report.txt - starting");
        assert!(pending_text.is_ascii());
        assert_eq!(
            operation_progress_value(pending),
            "1 active file operation, progress pending"
        );

        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(180.0, 36.0), Sense::hover());
                paint_operation_progress_badge(ui, rect, progress, true, false);
            });
        });
        assert!(
            !out.shapes.is_empty(),
            "operation progress badge must paint visible shapes"
        );
    }

    #[test]
    fn pending_operation_progress_has_no_fabricated_fill() {
        let pending = OperationProgressView::new(1, None, "Copy report.txt");
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(180.0, 36.0), Sense::hover());
                paint_operation_progress_badge(ui, rect, pending, false, false);
            });
        });

        fn contains_fill(shape: &egui::Shape, color: egui::Color32) -> bool {
            match shape {
                egui::Shape::Rect(rect) => rect.fill == color,
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains_fill(shape, color)),
                _ => false,
            }
        }

        assert!(
            !out.shapes
                .iter()
                .any(|clipped| contains_fill(&clipped.shape, Style::TEXT_DIM)),
            "pending progress must leave the track empty instead of painting a guessed fill"
        );
    }

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != egui::Color32::PLACEHOLDER)
                        .then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn operation_progress_badge_keeps_status_visible_with_long_labels() {
        let progress =
            OperationProgressView::new(4, Some(0.87), "Synchronizing quarterly archive bundle");
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(128.0, 36.0), Sense::hover());
                paint_operation_progress_badge(ui, rect, progress, false, false);
            });
        });
        let texts = painted_text(&out.shapes);

        assert!(
            texts
                .iter()
                .any(|(text, color)| text == "87%" && *color == Style::ACCENT),
            "progress percent must remain a separate visible text chip: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == progress.label && *color == Style::TEXT),
            "operation label should still paint through the shared badge: {texts:?}"
        );
    }

    // --- UI-VIS-111: shared surface primitives -------------------------------

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn corner_maps_radius_tokens_to_egui_corner_radius() {
        assert_eq!(
            corner(Style::RADIUS_S),
            CornerRadius::same(Style::RADIUS_S as u8)
        );
        assert_eq!(
            corner(Style::RADIUS_M),
            CornerRadius::same(Style::RADIUS_M as u8)
        );
        assert_eq!(
            corner(Style::RADIUS_L),
            CornerRadius::same(Style::RADIUS_L as u8)
        );
        // Rounds and clamps rather than wrapping on an out-of-range radius.
        assert_eq!(corner(999.0), CornerRadius::same(255));
        assert_eq!(corner(-4.0), CornerRadius::same(0));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn surface_primitives_carry_the_shared_tokens() {
        // Card: a raised, bordered, mid-rounded surface that lifts off the page.
        let card = card();
        assert_eq!(card.fill, Style::SURFACE);
        assert_eq!(card.stroke, Style::hairline());
        assert_eq!(card.corner_radius, corner(Style::RADIUS_M));
        assert!(card.shadow.blur > 0, "a card casts a soft raised shadow");

        // Inset: recessed into the app background, tight radius, no shadow.
        let inset = inset();
        assert_eq!(inset.fill, Style::BG);
        assert_eq!(inset.corner_radius, corner(Style::RADIUS_S));
        assert_eq!(inset.shadow.color.a(), 0, "an inset casts no shadow");

        // Toolbar: quiet — filled but borderless and flat (UI-VIS-109).
        let toolbar = toolbar();
        assert_eq!(toolbar.fill, Style::SURFACE);
        assert_eq!(
            toolbar.stroke.width, 0.0,
            "an inactive toolbar has no border"
        );
        assert_eq!(toolbar.shadow.color.a(), 0, "a toolbar is flat");

        // Section: typographic, not a box — no fill, no stroke, no shadow.
        let section = section();
        assert_eq!(section.fill.a(), 0, "a section is unfilled (no box)");
        assert_eq!(section.stroke.width, 0.0, "a section has no border");
        assert_eq!(section.shadow.color.a(), 0, "a section casts no shadow");

        // Dialog: the large radius and the deepest (modal) shadow.
        let dialog = dialog();
        assert_eq!(dialog.corner_radius, corner(Style::RADIUS_L));
        assert_eq!(dialog.shadow, Elevation::Modal.egui_shadow());

        // Overlay: mid radius, the floating overlay shadow.
        let overlay = overlay();
        assert_eq!(overlay.corner_radius, corner(Style::RADIUS_M));
        assert_eq!(overlay.shadow, Elevation::Overlay.egui_shadow());

        // The elevation ladder is honoured across the primitives: card < overlay <
        // dialog in shadow depth.
        assert!(card.shadow.blur < overlay.shadow.blur);
        assert!(overlay.shadow.blur < dialog.shadow.blur);
    }

    #[test]
    fn surface_primitives_lay_out_headless() {
        // Every primitive must actually render a real frame (shapes, no panic) on a
        // CPU-only context — proof they are live, reusable chrome, not dead code.
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for frame in [card(), inset(), toolbar(), section(), dialog(), overlay()] {
                    frame.show(ui, |ui| {
                        ui.label("content");
                    });
                }
            });
        });
        assert!(
            !out.shapes.is_empty(),
            "surface primitives must paint visible shapes"
        );
    }

    #[test]
    fn tooltip_uses_the_shared_overlay_and_typography() {
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                tooltip(ui, "Shared tooltip");
            });
        });
        assert!(!out.shapes.is_empty(), "tooltip must paint visible shapes");
        let frame = overlay();
        assert_eq!(frame.fill, Style::SURFACE);
        assert_eq!(frame.stroke, Style::hairline());
        assert_eq!(frame.corner_radius, corner(Style::RADIUS_M));
    }
}
