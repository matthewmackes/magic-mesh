//! PLANES-19 — Network ▸ Routing panel.
//!
//! The overlay-reachability validation surface (W79/W80): the
//! validation suite probes every directed edge between participants over
//! the Nebula overlay; an edge that never came back reachable is a
//! failure that feeds the drift pipeline (W80). This panel shells
//! `mackesd validate status --json` to show the newest run's verdict and
//! `mackesd validate run` to request a fresh one (the FPG leader mints
//! it). Routing itself stays display-only (W76) — what the operator acts
//! on here is the reachability health.

use std::time::{Duration, SystemTime};

use cosmic::iced::widget::{button, column, container, pick_list, row, scrollable, text, Space};
use cosmic::iced::Task;

/// ROUTING-VALIDATE-1 — after requesting a run, poll the verdict on this cadence
/// until it lands (the FPG leader mints it + nodes report asynchronously, so the
/// result isn't ready on the first immediate fetch).
const POLL_DELAY: Duration = Duration::from_secs(3);
/// Bounded so a leader that never completes can't poll forever.
const MAX_POLLS: u8 = 12;
/// ROUTE-TRACE-4 — read budget for the `action/route/trace` Bus probe. Matches
/// the other panels' interactive 2 s read window (the responder assembles the
/// graph from local exposure/peer state — no network round-trips).
const TRACE_TIMEOUT: Duration = Duration::from_secs(2);
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Theme};
use mackes_mesh_types::route_trace::{
    ControlPoint, Direction, Layer, NodeKind, PathEdge, PathGraph, Verdict,
};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::BadgeSeverity;

/// One directed `from → to` edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

/// The newest validation run's verdict, parsed from
/// `mackesd validate status --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationStatus {
    /// `None` when no run has been minted yet.
    pub run_id: Option<String>,
    pub passed: bool,
    pub reachable: usize,
    pub failed_edges: Vec<Edge>,
    pub missing_reporters: Vec<String>,
}

/// ROUTE-TRACE-4 — wire values for the Egress/Ingress direction toggle, in the
/// kebab-case the `action/route/trace` IPC + `route_trace::Direction` expect.
const DIRECTION_CHOICES: [&str; 2] = ["egress", "ingress"];

#[derive(Debug, Clone, Default)]
pub struct RoutingPanel {
    pub status: ValidationStatus,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
    pub run_result: Option<Result<String, String>>,
    /// AUDIT-MESH-5 — guards the one-shot auto-run: when the panel opens and no
    /// validation run has ever been minted, it requests one automatically (so
    /// Routing shows live reachability without a manual click), but only once
    /// per panel session — a genuinely empty mesh won't re-probe on every load.
    pub auto_ran: bool,
    /// ROUTING-VALIDATE-1 — how many times we've polled the verdict since the
    /// last run request (bounded by `MAX_POLLS`).
    pub poll_attempts: u8,
    /// ROUTE-TRACE-4 — the path-trace toolbar state machine.
    pub trace: TraceState,
}

/// ROUTE-TRACE-4 — the trace toolbar's selection state + last result.
///
/// The toolbar lets the operator pick a **source node**, a **destination
/// service/host**, and an **Egress/Ingress direction**, then run a trace. The
/// pure [`TraceState::request_body`] turns that selection into the exact
/// `action/route/trace` request shape the responder (`mackesd/src/ipc/route.rs`)
/// expects; [`TraceState::can_trace`] is the button's enable gate. The rendered
/// [`PathGraph`] (or an error) lands in `result`.
#[derive(Debug, Clone, Default)]
pub struct TraceState {
    /// Source-node label (the egress originator). Egress requires it; ingress
    /// ignores it (the responder resolves the host from the service's policy).
    pub source: String,
    /// Destination — a service id (ingress) or an external host/IP (egress).
    pub dest: String,
    /// `"egress"` | `"ingress"` (the toggle's wire value; default egress).
    pub direction: String,
    /// True while an `action/route/trace` request is in flight.
    pub busy: bool,
    /// The most recent trace result: the rendered `PathGraph`, or an error.
    pub result: Option<Result<PathGraph, String>>,
}

impl TraceState {
    /// Which direction the toggle currently selects (defaults to Egress for a
    /// blank/unknown wire value, matching `route_trace::Direction::default`).
    #[must_use]
    pub fn dir(&self) -> Direction {
        match self.direction.as_str() {
            "ingress" => Direction::Ingress,
            _ => Direction::Egress,
        }
    }

    /// True when the current selection is complete enough to trace. Egress needs
    /// a source node (the responder errors without a `from`); ingress needs a
    /// destination service id (the responder errors without a `to`). Never busy.
    #[must_use]
    pub fn can_trace(&self) -> bool {
        if self.busy {
            return false;
        }
        match self.dir() {
            Direction::Egress => !self.source.trim().is_empty(),
            Direction::Ingress => !self.dest.trim().is_empty(),
        }
    }

    /// Build the exact `action/route/trace` request body for the current
    /// selection (pure — the toolbar state machine's core, unit-tested). The
    /// responder reads `{ direction, from, to }`:
    ///
    /// * egress — `from` = source node, `to` = external dest (blank ⇒ the
    ///   responder defaults it to "Internet");
    /// * ingress — `to` = the service id (`from` is unused).
    ///
    /// Returns `None` when the selection isn't traceable yet
    /// ([`Self::can_trace`] is false), so the caller never publishes an
    /// under-specified request.
    #[must_use]
    pub fn request_body(&self) -> Option<String> {
        if !self.can_trace() {
            return None;
        }
        let body = match self.dir() {
            Direction::Egress => serde_json::json!({
                "direction": "egress",
                "from": self.source.trim(),
                "to": self.dest.trim(),
            }),
            Direction::Ingress => serde_json::json!({
                "direction": "ingress",
                "to": self.dest.trim(),
            }),
        };
        Some(body.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<ValidationStatus, String>),
    RefreshClicked,
    RunNow,
    RunRequested(Result<String, String>),
    /// ROUTE-TRACE-4 — trace toolbar edits.
    TraceSourceChanged(String),
    TraceDestChanged(String),
    TraceDirectionSelected(String),
    TraceClicked,
    /// ROUTE-TRACE-4 — an `action/route/trace` reply landed (the `PathGraph` or
    /// an error message).
    TraceLoaded(Result<PathGraph, String>),
}

impl RoutingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_status() }, |result| {
            crate::Message::Routing(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(status)) => {
                let never_run = status.run_id.is_none();
                self.status = status;
                self.error = None;
                self.last_run_at = Some(SystemTime::now());
                // A verdict landed — stop polling.
                if !never_run {
                    self.busy = false;
                    self.poll_attempts = 0;
                    return Task::none();
                }
                // AUDIT-MESH-5 — no run has ever been minted: auto-request one
                // (once) so the panel shows live reachability without the
                // operator having to click "Run validation now".
                if !self.auto_ran {
                    self.auto_ran = true;
                    self.busy = true;
                    self.poll_attempts = 0;
                    self.run_result = None;
                    return Task::perform(async { request_run() }, |result| {
                        crate::Message::Routing(Message::RunRequested(result))
                    });
                }
                // ROUTING-VALIDATE-1 — a run was requested but the verdict isn't
                // ready yet (the leader mints it + nodes report async). Keep
                // polling on a cadence until it lands or the budget is spent —
                // before, the panel fetched once, saw nothing, and gave up
                // ("No validation run yet" forever).
                if self.busy && self.poll_attempts < MAX_POLLS {
                    self.poll_attempts += 1;
                    return poll_status_later();
                }
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.status = ValidationStatus::default();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.run_result = None;
                Self::load()
            }
            Message::RunNow => {
                self.busy = true;
                self.poll_attempts = 0;
                Task::perform(async { request_run() }, |result| {
                    crate::Message::Routing(Message::RunRequested(result))
                })
            }
            Message::RunRequested(result) => {
                self.run_result = Some(result);
                // Keep busy + poll for the freshly-minted verdict rather than
                // fetching once immediately (it isn't ready yet).
                self.busy = true;
                self.poll_attempts = 0;
                poll_status_later()
            }
            Message::TraceSourceChanged(v) => {
                self.trace.source = v;
                Task::none()
            }
            Message::TraceDestChanged(v) => {
                self.trace.dest = v;
                Task::none()
            }
            Message::TraceDirectionSelected(v) => {
                self.trace.direction = v;
                Task::none()
            }
            Message::TraceClicked => {
                // Build the request body from the toolbar state; a noop if the
                // selection isn't traceable yet (the button is disabled in that
                // case, but guard regardless).
                let Some(body) = self.trace.request_body() else {
                    return Task::none();
                };
                self.trace.busy = true;
                Task::perform(
                    async move { tokio::task::spawn_blocking(move || request_trace(&body)).await },
                    |joined| {
                        let result = joined.unwrap_or_else(|e| Err(format!("trace task: {e}")));
                        crate::Message::Routing(Message::TraceLoaded(result))
                    },
                )
            }
            Message::TraceLoaded(result) => {
                self.trace.busy = false;
                self.trace.result = Some(result);
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Routing")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("overlay-reachability validation")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let style_btn = move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            let bg = match status {
                cosmic::iced::widget::button::Status::Hovered => Color {
                    r: accent.r * 1.10,
                    g: accent.g * 1.10,
                    b: accent.b * 1.10,
                    a: accent.a,
                },
                _ => accent,
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 6.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        };
        let run_btn = button(text("Run validation now").size(13).colr(Color::WHITE))
            .padding(Padding::from([6u16, 14u16]))
            .sty(style_btn)
            .on_press(crate::Message::Routing(Message::RunNow));
        let refresh_btn = button(
            text(if self.busy { "…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(style_btn)
        .on_press(crate::Message::Routing(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            run_btn,
            Space::new().width(Length::Fixed(8.0)),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut body_col = column![].spacing(6);
        // ROUTE-TRACE-4 — the path-trace toolbar + topology graph sit atop the
        // reachability verdict (the trace is the interactive "why is this path
        // (un)reachable" lens over the same overlay state).
        body_col = body_col.push(trace_toolbar(&self.trace, palette));
        body_col = body_col.push(trace_graph(&self.trace, palette));
        if let Some(res) = &self.run_result {
            body_col = body_col.push(result_strip(res, palette));
        }
        if self.last_run_at.is_some() {
            if self.status.run_id.is_some() {
                body_col = body_col.push(verdict_card(&self.status, palette));
                for e in &self.status.failed_edges {
                    body_col = body_col.push(failed_edge_row(e, palette));
                }
            } else {
                body_col =
                    body_col.push(empty_state_card(palette, self.error.as_deref(), self.busy));
            }
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(body_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn verdict_card<'a>(s: &ValidationStatus, palette: Palette) -> Element<'a, crate::Message> {
    let (icon, color, label) = if s.passed {
        (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "PASS — every overlay edge reachable".to_string(),
        )
    } else {
        (
            Icon::StatusError,
            palette.danger.into_cosmic_color(),
            format!(
                "FAIL — {} unreachable edge{}, {} missing reporter{}",
                s.failed_edges.len(),
                if s.failed_edges.len() == 1 { "" } else { "s" },
                s.missing_reporters.len(),
                if s.missing_reporters.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )
    };
    let resolved = mde_icon(icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph).size(16.0).colr(color).into()
    };
    let head = row![
        icon_widget,
        text(label).size(12).colr(color),
        Space::new().width(Length::Fill),
        text(format!("{} reachable", s.reachable))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let rid = s.run_id.clone().unwrap_or_default();
    card(
        column![
            head,
            text(format!("run {rid}"))
                .size(10)
                .colr(palette.text_muted.into_cosmic_color())
        ]
        .spacing(4),
        palette,
    )
}

fn failed_edge_row<'a>(e: &Edge, palette: Palette) -> Element<'a, crate::Message> {
    let danger = palette.danger.into_cosmic_color();
    card(
        row![
            text(format!("{} → {}", e.from, e.to))
                .size(12)
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fill),
            text("unreachable").size(11).colr(danger),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center),
        palette,
    )
}

fn result_strip<'a>(res: &Result<String, String>, palette: Palette) -> Element<'a, crate::Message> {
    let (color, label) = match res {
        Ok(msg) => (palette.success.into_cosmic_color(), msg.clone()),
        Err(e) => (palette.danger.into_cosmic_color(), format!("error — {e}")),
    };
    let bg = palette.raised.into_cosmic_color();
    container(text(label).size(11).colr(color))
        .padding(Padding::from([8u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn empty_state_card<'a>(
    palette: Palette,
    error: Option<&'a str>,
    busy: bool,
) -> Element<'a, crate::Message> {
    let (icon_kind, icon_color, heading, body): (Icon, Color, String, String) =
        if let Some(err) = error {
            (
                Icon::StatusError,
                palette.danger.into_cosmic_color(),
                "Couldn't read validation".to_string(),
                err.to_string(),
            )
        } else if busy {
            // AUDIT-MESH-5 — the one-shot auto-run is in flight.
            (
                Icon::Network,
                palette.accent.into_cosmic_color(),
                "Running validation…".to_string(),
                "Probing every directed overlay edge between participants — the \
                 FPG leader mints the run and each node reports what it could \
                 reach. The verdict appears here as soon as the reporters return."
                    .to_string(),
            )
        } else {
            (
                Icon::Network,
                palette.accent.into_cosmic_color(),
                "No validation run yet".to_string(),
                "The overlay-reachability suite probes every directed edge between \
                 participants. Click \"Run validation now\" to request a run — the FPG \
                 leader mints it, every node reports what it could reach, and the verdict \
                 (with any unreachable edges) appears here."
                    .to_string(),
            )
        };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(32.0)
            .colr(icon_color)
            .into()
    };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text(heading)
                .size(14)
                .colr(palette.text.into_cosmic_color()),
            text(body)
                .size(11)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

fn card<'a>(
    inner: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(inner)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ---- ROUTE-TRACE-4: trace toolbar + topology graph ------------------------

/// ROUTE-TRACE-4 — the trace toolbar: a source-node picker, a destination
/// service/host field, an Egress/Ingress direction toggle, and a Trace button.
/// The direction toggle re-labels the fields' meaning (egress traces a node's
/// WAN path to a host; ingress traces an external client's path to a published
/// service) and flips which field gates the Trace button.
fn trace_toolbar<'a>(state: &'a TraceState, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let dir = state.dir();
    let label = move |s: &str| {
        text(s.to_string())
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(70.0))
    };

    // Source node — meaningful for egress (the originating mesh node); ingress
    // resolves the host from the service policy, so it's shown as a muted note
    // (not an editable field) in that direction. The editable fields use the
    // shared Carbon-token input chrome (`controls::styled_text_input`) so they
    // match every other panel's inputs (§4).
    let dest_hint = match dir {
        Direction::Egress => "destination host/IP (e.g. 1.1.1.1)",
        Direction::Ingress => "service id (e.g. grafana)",
    };
    // `controls::styled_text_input` returns a `cosmic::Theme` element (the same
    // theme as the surrounding panel tree), so it drops straight in — no `themer`
    // bridge needed (unlike the stock-iced-themed canvas).
    let source_widget: Element<'a, crate::Message> = match dir {
        Direction::Egress => crate::controls::styled_text_input(
            "source node (e.g. eagle)",
            &state.source,
            |v| crate::Message::Routing(Message::TraceSourceChanged(v)),
            palette,
        ),
        Direction::Ingress => text("(resolved from the service's policy)")
            .size(13)
            .colr(palette.text_muted.into_cosmic_color())
            .into(),
    };
    let dest_widget = crate::controls::styled_text_input(
        dest_hint,
        &state.dest,
        |v| crate::Message::Routing(Message::TraceDestChanged(v)),
        palette,
    );

    let direction_picker = pick_list(
        DIRECTION_CHOICES.map(String::from).to_vec(),
        Some(if state.direction.is_empty() {
            "egress".to_string()
        } else {
            state.direction.clone()
        }),
        |v| crate::Message::Routing(Message::TraceDirectionSelected(v)),
    )
    .text_size(13);

    let trace_msg = state
        .can_trace()
        .then_some(crate::Message::Routing(Message::TraceClicked));
    let trace_btn = crate::controls::variant_button(
        if state.busy { "Tracing…" } else { "Trace" },
        crate::controls::ButtonVariant::Primary,
        trace_msg,
        palette,
    );

    let title = text("Trace a path")
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text.into_cosmic_color());

    let source_row = row![label("From"), source_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let dest_row = row![label("To"), dest_widget]
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    let controls_row = row![
        label("Direction"),
        direction_picker,
        Space::new().width(Length::Fill),
        trace_btn,
    ]
    .spacing(10)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    card(
        column![title, source_row, dest_row, controls_row].spacing(8),
        palette,
    )
}

/// ROUTE-TRACE-4 — the topology-graph card: renders the most recent trace's
/// `PathGraph` on a canvas (node glyphs by kind, edge color by layer, RTT/loss
/// labels), or a hint / error when nothing has been traced yet. Reuses the
/// canvas drawing approach from the Peers map (`peers_map::MapProgram`).
fn trace_graph<'a>(state: &TraceState, palette: Palette) -> Element<'a, crate::Message> {
    match &state.result {
        Some(Ok(graph)) => {
            // The canvas program paints from `palette` (it ignores the passed
            // stock theme), so `themer(None, ...)` bridges the stock-themed
            // canvas into the surrounding cosmic theme — same pattern as Peers.
            let program = PathGraphProgram {
                graph: graph.clone(),
                palette,
            };
            let canvas_stock: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
                cosmic::iced::widget::canvas(program)
                    .width(Length::Fill)
                    .height(Length::Fixed(280.0))
                    .into();
            let canvas: Element<'_, crate::Message> =
                cosmic::iced::widget::themer(None, canvas_stock).into();
            let verdict = path_verdict_line(graph, palette);
            let mut body = column![verdict, container(canvas).width(Length::Fill)].spacing(8);
            // ROUTE-TRACE-5 — the per-hop control list under the canvas: one row
            // per edge that crosses a firewall/control point, each with a
            // tone-tinted verdict badge + the cited rule. The canvas shows *where*
            // the path stops; this list says *why*, citing each control's rule.
            if let Some(controls) = control_hops_list(graph, palette) {
                body = body.push(controls);
            }
            card(body, palette)
        }
        Some(Err(e)) => card(
            text(format!("Trace failed — {e}"))
                .size(12)
                .colr(palette.danger.into_cosmic_color()),
            palette,
        ),
        None => card(
            text(
                "Pick a source + destination and a direction, then Trace to render the path \
                 graph — node glyphs by kind, edges colored by layer with RTT/loss labels, the \
                 first blocking control point highlighted.",
            )
            .size(12)
            .colr(palette.text_muted.into_cosmic_color()),
            palette,
        ),
    }
}

/// ROUTE-TRACE-4 — a one-line verdict over the rendered path: reachable, blocked
/// (citing where), or indeterminate (a control point couldn't be resolved).
fn path_verdict_line<'a>(graph: &PathGraph, palette: Palette) -> Element<'a, crate::Message> {
    let (color, label) = if let Some(at) = &graph.blocked_at {
        (
            palette.danger.into_cosmic_color(),
            format!("BLOCKED at {}", blocked_edge_label(graph, at)),
        )
    } else if graph.has_indeterminate() {
        (
            palette.warning.into_cosmic_color(),
            "INDETERMINATE — a control point couldn't be resolved".to_string(),
        )
    } else {
        (
            palette.success.into_cosmic_color(),
            "REACHABLE — the path reaches its destination unblocked".to_string(),
        )
    };
    let dir = match graph.direction {
        Direction::Egress => "egress",
        Direction::Ingress => "ingress",
    };
    row![
        text(label).size(12).colr(color),
        Space::new().width(Length::Fill),
        text(format!("{dir} · {} hops", graph.edges.len()))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// ROUTE-TRACE-4 — render the blocked edge id (`<from-id>-><to-id>`) as a human
/// `<from-label> → <to-label>` using the graph's node labels, so the verdict
/// reads like the graph (hostnames/service names) rather than the internal wire
/// ids. Falls back to the raw edge id if the edge isn't found.
fn blocked_edge_label(graph: &PathGraph, edge_id: &str) -> String {
    let label_of = |id: &str| -> String {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map_or_else(|| id.to_string(), |n| n.label.clone())
    };
    graph.edges.iter().find(|e| e.id() == edge_id).map_or_else(
        || edge_id.to_string(),
        |e| format!("{} → {}", label_of(&e.from), label_of(&e.to)),
    )
}

// ---- ROUTE-TRACE-5: per-hop control-point list ----------------------------

/// ROUTE-TRACE-5 — the shared severity tone a control-point [`Verdict`] reads
/// as, on the Carbon support ramp (no raw hex — §4): Allow is success (the
/// segment is permitted), Block is danger (the path stops here), Indeterminate
/// is warning (the rule set couldn't be resolved — never guessed). This is the
/// 1:1 verdict→[`BadgeSeverity`] mapping the firewall badge tints from; pure +
/// unit-tested so the color derivation is verifiable without rendering.
#[must_use]
fn verdict_severity(verdict: Verdict) -> BadgeSeverity {
    match verdict {
        Verdict::Allow => BadgeSeverity::Success,
        Verdict::Block => BadgeSeverity::Danger,
        Verdict::Indeterminate => BadgeSeverity::Warning,
    }
}

/// ROUTE-TRACE-5 — the short, uppercase label a [`Verdict`] shows on its badge.
#[must_use]
fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Allow => "ALLOW",
        Verdict::Block => "BLOCK",
        Verdict::Indeterminate => "INDET",
    }
}

/// ROUTE-TRACE-5 — a tone-tinted firewall badge for a control point, so
/// Allow/Block/Indeterminate read as a glanceable green/red/amber chip. Reuses
/// the shared [`panel_chrome::status_badge`] (the same severity-tinted pill every
/// other panel uses) tinted from `control.verdict` via [`verdict_severity`] — one
/// badge chrome, sourced from Carbon tokens, no raw hex.
fn firewall_badge<'a>(control: &ControlPoint, palette: Palette) -> Element<'a, crate::Message> {
    crate::panel_chrome::status_badge(
        verdict_label(control.verdict),
        verdict_severity(control.verdict),
        palette,
    )
}

/// ROUTE-TRACE-5 — one control-point hop row: the tone-tinted [`firewall_badge`],
/// the human `<from> → <to>` segment, and a small drill-down detail line under it
/// citing the control (`firewall` name) and its `rule`. The blocking hop (the one
/// `blocked_at` points at) is marked so the operator can read the per-hop list as
/// the canvas's explanation. The detail line is the per-hop drill-down — the wire
/// firewall id + the exact cited rule, the "why" behind the badge.
fn control_hop_row<'a>(
    graph: &PathGraph,
    edge: &PathEdge,
    palette: Palette,
) -> Option<Element<'a, crate::Message>> {
    let control = edge.control.as_ref()?;
    let label_of = |id: &str| -> String {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map_or_else(|| id.to_string(), |n| n.label.clone())
    };
    let badge = firewall_badge(control, palette);
    let is_blocking = graph.blocked_at.as_deref() == Some(edge.id().as_str());

    let segment = text(format!("{} → {}", label_of(&edge.from), label_of(&edge.to)))
        .size(12)
        .colr(palette.text.into_cosmic_color());

    let mut head = row![badge, segment]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);
    if is_blocking {
        // The first denying point — flag it so this row reads as the canvas's
        // BLOCKED highlight in list form.
        head = head.push(Space::new().width(Length::Fill));
        head = head.push(
            text("first block")
                .size(10)
                .colr(palette.danger.into_cosmic_color()),
        );
    }

    // The per-hop drill-down: the control's name + the exact cited rule — the
    // detail behind the badge ("firewalld:public · default deny (no matching rule)").
    let detail = text(format!("{} · {}", control.firewall, control.rule))
        .size(10)
        .colr(palette.text_muted.into_cosmic_color());

    Some(card(column![head, detail].spacing(4), palette))
}

/// ROUTE-TRACE-5 — the per-hop control list: one [`control_hop_row`] for each
/// edge that crosses a control point (a [`ControlPoint`]), in source→dest order,
/// under a small heading. Returns `None` when the path crosses no control points
/// (a plain egress with no modeled firewall) — the canvas alone suffices then, so
/// no empty list chrome is rendered.
fn control_hops_list<'a>(
    graph: &PathGraph,
    palette: Palette,
) -> Option<Element<'a, crate::Message>> {
    let hop_rows: Vec<Element<'a, crate::Message>> = graph
        .edges
        .iter()
        .filter_map(|edge| control_hop_row(graph, edge, palette))
        .collect();
    if hop_rows.is_empty() {
        return None;
    }
    let mut rows = column![].spacing(6);
    for r in hop_rows {
        rows = rows.push(r);
    }
    let heading = text("Control points")
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    Some(column![heading, rows].spacing(6).into())
}

/// ROUTE-TRACE-4 — the topology-graph canvas program. Lays the path out as a
/// horizontal chain (source→dest order — a path is linear), draws each edge
/// colored by its [`Layer`] with an RTT/loss label, highlights the active path
/// (and the first blocking edge in danger), and paints each node as a glyph
/// sized/colored by its [`NodeKind`]. Paints from `palette` (Carbon tokens) — no
/// raw hex.
struct PathGraphProgram {
    graph: PathGraph,
    palette: Palette,
}

impl PathGraphProgram {
    /// Project the path's nodes onto a horizontal chain across `bounds`, in
    /// source→dest order (a [`PathGraph`] is a linear path). Returns id→point.
    fn projected(
        &self,
        bounds: &cosmic::iced::Rectangle,
    ) -> std::collections::HashMap<String, cosmic::iced::Point> {
        use cosmic::iced::Point;
        let n = self.graph.nodes.len().max(1);
        let pad = 60.0_f32;
        let usable = (bounds.width - pad * 2.0).max(1.0);
        let step = if n > 1 { usable / (n - 1) as f32 } else { 0.0 };
        let y = bounds.height / 2.0;
        self.graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.id.clone(), Point::new(pad + step * i as f32, y)))
            .collect()
    }
}

/// ROUTE-TRACE-4 — the Carbon token an edge's [`Layer`] colors to. Host=muted
/// (local), Mesh=accent (the overlay), Vpn=warning (a tunnel boundary),
/// Public=success-but-it's-really-just-"open" — we use `text` for public so the
/// four layers read distinctly against the canvas. A blocked edge overrides this
/// with `danger` at the draw site.
fn layer_color(layer: Layer, palette: Palette) -> Color {
    match layer {
        Layer::Host => palette.text_muted.into_cosmic_color(),
        Layer::Mesh => palette.accent.into_cosmic_color(),
        Layer::Vpn => palette.warning.into_cosmic_color(),
        Layer::Public => palette.text.into_cosmic_color(),
    }
}

/// ROUTE-TRACE-4 — a short glyph for a node [`NodeKind`] (drawn inside the node
/// disc). Plain ASCII so it renders without an icon font on the canvas.
fn node_glyph(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Host => "H",
        NodeKind::Vm => "V",
        NodeKind::Container => "C",
        NodeKind::OverlayPeer => "P",
        NodeKind::Gateway => "G",
        NodeKind::VpnExit => "X",
        NodeKind::Ingress => "I",
        NodeKind::Internet => "@",
        NodeKind::Service => "S",
    }
}

impl cosmic::iced::widget::canvas::Program<crate::Message> for PathGraphProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &cosmic::iced::Renderer,
        _theme: &cosmic::iced::Theme,
        bounds: cosmic::iced::Rectangle,
        _cursor: cosmic::iced::mouse::Cursor,
    ) -> Vec<cosmic::iced::widget::canvas::Geometry> {
        use cosmic::iced::alignment::Vertical;
        use cosmic::iced::widget::canvas::{Frame, Path, Stroke, Text};
        use cosmic::iced::widget::text::Alignment;
        use cosmic::iced::{Pixels, Point, Rectangle};
        let mut frame = Frame::new(renderer, bounds.size());
        let rect = Rectangle::with_size(bounds.size());
        let proj = self.projected(&rect);
        let p = &self.palette;

        // Edges first (under the nodes), colored by layer; the blocking edge is
        // drawn in danger + thicker to highlight where the path stops.
        for edge in &self.graph.edges {
            let (Some(&from), Some(&to)) = (proj.get(&edge.from), proj.get(&edge.to)) else {
                continue;
            };
            let blocked = self
                .graph
                .blocked_at
                .as_deref()
                .is_some_and(|b| b == edge.id());
            let indeterminate = edge
                .control
                .as_ref()
                .is_some_and(|c| c.verdict == Verdict::Indeterminate);
            let (color, width) = if blocked {
                (p.danger.into_cosmic_color(), 3.0)
            } else if indeterminate {
                (p.warning.into_cosmic_color(), 2.0)
            } else {
                (layer_color(edge.layer, *p), 2.0)
            };
            frame.stroke(
                &Path::line(from, to),
                Stroke::default().with_color(color).with_width(width),
            );
            // RTT/loss label above the segment midpoint; layer name below it so
            // the operator can read the edge's layer at a glance.
            let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
            // Only render finite measurements; a non-finite probe value is
            // dropped rather than shown as "NaN% loss". Loss is a 0.0..=1.0
            // fraction (the route_trace model contract) → clamp before %.
            let rtt = edge.rtt_ms.filter(|v| v.is_finite());
            let loss = edge
                .loss
                .filter(|v| v.is_finite())
                .map(|v| (v.clamp(0.0, 1.0)) * 100.0);
            let metric = match (rtt, loss) {
                (Some(rtt), Some(loss)) => format!("{rtt:.0} ms · {loss:.0}% loss"),
                (Some(rtt), None) => format!("{rtt:.0} ms"),
                (None, Some(loss)) => format!("{loss:.0}% loss"),
                (None, None) => String::new(),
            };
            if !metric.is_empty() {
                frame.fill_text(Text {
                    content: metric,
                    position: Point::new(mid.x, mid.y - 16.0),
                    color: p.text.into_cosmic_color(),
                    size: Pixels(10.0),
                    align_x: Alignment::Center,
                    ..Text::default()
                });
            }
            frame.fill_text(Text {
                content: layer_label(edge.layer).to_string(),
                position: Point::new(mid.x, mid.y + 6.0),
                color,
                size: Pixels(9.0),
                align_x: Alignment::Center,
                ..Text::default()
            });
        }

        // Nodes: a disc with the kind glyph, the label below.
        for node in &self.graph.nodes {
            let Some(&at) = proj.get(&node.id) else {
                continue;
            };
            let r = 14.0;
            frame.fill(&Path::circle(at, r), p.surface.into_cosmic_color());
            frame.stroke(
                &Path::circle(at, r),
                Stroke::default()
                    .with_color(p.accent.into_cosmic_color())
                    .with_width(1.5),
            );
            frame.fill_text(Text {
                content: node_glyph(node.kind).to_string(),
                position: at,
                color: p.text.into_cosmic_color(),
                size: Pixels(13.0),
                align_x: Alignment::Center,
                align_y: Vertical::Center,
                ..Text::default()
            });
            frame.fill_text(Text {
                content: node.label.clone(),
                position: Point::new(at.x, at.y + r + 6.0),
                color: p.text_muted.into_cosmic_color(),
                size: Pixels(11.0),
                align_x: Alignment::Center,
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// ROUTE-TRACE-4 — the short layer name drawn under each edge.
fn layer_label(layer: Layer) -> &'static str {
    match layer {
        Layer::Host => "host",
        Layer::Mesh => "mesh",
        Layer::Vpn => "vpn",
        Layer::Public => "public",
    }
}

// ---- I/O ------------------------------------------------------

/// ROUTE-TRACE-4 — request a path trace over the Bus (`action/route/trace`) and
/// decode the reply into a [`PathGraph`]. The responder replies
/// `{"ok":true,"graph":<PathGraph>}` on success or `{"error":...}` on failure.
/// Blocking (the Bus client builds its own current-thread runtime) — call from
/// `spawn_blocking`, never on the iced executor.
fn request_trace(body: &str) -> Result<PathGraph, String> {
    let raw =
        crate::dbus::action_request_with_body("action/route/trace", Some(body), TRACE_TIMEOUT)
            .ok_or_else(|| "mackesd not reachable over the Bus (route/trace)".to_string())?;
    parse_trace_reply(&raw)
}

/// ROUTE-TRACE-4 — pure decoder for the `action/route/trace` reply envelope:
/// `{"ok":true,"graph":<PathGraph>}` → the graph; `{"error":m}` → `Err(m)`;
/// anything else → a "bad reply" error. Split out so the wire contract is
/// unit-testable without the Bus.
fn parse_trace_reply(raw: &str) -> Result<PathGraph, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad trace reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let graph = v
        .get("graph")
        .ok_or_else(|| "trace reply missing 'graph'".to_string())?;
    serde_json::from_value::<PathGraph>(graph.clone())
        .map_err(|e| format!("trace reply decode: {e}"))
}

/// ROUTING-VALIDATE-1 — sleep `POLL_DELAY`, then re-fetch the verdict. Used to
/// poll for a freshly-requested run's result (the leader mints it + nodes report
/// asynchronously, so it isn't ready on the immediate fetch).
fn poll_status_later() -> Task<crate::Message> {
    Task::perform(
        async {
            tokio::time::sleep(POLL_DELAY).await;
            fetch_status()
        },
        |result| crate::Message::Routing(Message::Loaded(result)),
    )
}

/// Shell out to `mackesd validate status --json`.
pub fn fetch_status() -> Result<ValidationStatus, String> {
    let out = std::process::Command::new("mackesd")
        .args(["validate", "status", "--json"])
        .output()
        .map_err(|e| format!("mackesd validate status failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd validate status exited non-zero: {stderr}"));
    }
    Ok(parse_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Shell out to `mackesd validate run` (request a fresh run).
pub fn request_run() -> Result<String, String> {
    let out = std::process::Command::new("mackesd")
        .args(["validate", "run"])
        .output()
        .map_err(|e| format!("mackesd validate run failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd validate run exited non-zero: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Pure parser for the `validate status --json` object.
#[must_use]
pub fn parse_status(raw: &str) -> ValidationStatus {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return ValidationStatus::default();
    };
    let run_id = v.get("run_id").and_then(|x| x.as_str()).map(str::to_string);
    let edges = |key: &str| -> Vec<Edge> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(Edge {
                            from: e.get("from")?.as_str()?.to_string(),
                            to: e.get("to")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    ValidationStatus {
        run_id,
        passed: v
            .get("passed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        reachable: v
            .get("reachable")
            .and_then(|x| x.as_array())
            .map_or(0, Vec::len),
        failed_edges: edges("failed"),
        missing_reporters: v
            .get("missing_reporters")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_reads_a_pass_verdict() {
        let raw = r#"{"run_id":"v-1","passed":true,"reachable":[{"from":"a","to":"b"}],
            "failed":[],"missing_reporters":[]}"#;
        let s = parse_status(raw);
        assert_eq!(s.run_id.as_deref(), Some("v-1"));
        assert!(s.passed);
        assert_eq!(s.reachable, 1);
        assert!(s.failed_edges.is_empty());
    }

    #[test]
    fn parse_status_reads_a_fail_verdict_with_edges() {
        let raw = r#"{"run_id":"v-2","passed":false,"reachable":[],
            "failed":[{"from":"pine","to":"oak"}],"missing_reporters":["birch"]}"#;
        let s = parse_status(raw);
        assert!(!s.passed);
        assert_eq!(s.failed_edges.len(), 1);
        assert_eq!(s.failed_edges[0].from, "pine");
        assert_eq!(s.missing_reporters, vec!["birch".to_string()]);
    }

    #[test]
    fn parse_status_handles_no_run_and_garbage() {
        assert!(parse_status(r#"{"run_id":null}"#).run_id.is_none());
        assert!(parse_status("not json").run_id.is_none());
    }

    #[test]
    fn no_prior_run_auto_runs_once_then_polls_bounded() {
        // AUDIT-MESH-5 + ROUTING-VALIDATE-1 — first load with run_id:null
        // auto-requests a run (busy + auto_ran set); subsequent empty loads do
        // NOT re-request, but they DO keep polling for the verdict until the
        // budget (MAX_POLLS) is spent, then stop.
        let mut p = RoutingPanel::new();
        assert!(!p.auto_ran);
        let none_status = parse_status(r#"{"run_id":null}"#);
        let _ = p.update(Message::Loaded(Ok(none_status.clone())));
        assert!(p.auto_ran, "auto-run armed on first empty load");
        assert!(p.busy, "auto-run is in flight");

        // Subsequent empty loads keep polling (busy stays) until the budget runs
        // out — the verdict isn't ready instantly (leader mints it async).
        for _ in 0..MAX_POLLS {
            let _ = p.update(Message::Loaded(Ok(none_status.clone())));
            assert!(p.auto_ran, "never re-arms a second request");
        }
        // One more empty load past the budget → polling stops.
        let _ = p.update(Message::Loaded(Ok(none_status)));
        assert!(!p.busy, "polling stops after MAX_POLLS empty loads");
    }

    #[test]
    fn verdict_arrival_stops_polling() {
        // ROUTING-VALIDATE-1 — once a run_id lands, polling stops + resets.
        let mut p = RoutingPanel::new();
        let _ = p.update(Message::Loaded(Ok(parse_status(r#"{"run_id":null}"#))));
        assert!(p.busy);
        let verdict = parse_status(r#"{"run_id":"r1","passed":true,"reachable":9}"#);
        let _ = p.update(Message::Loaded(Ok(verdict)));
        assert!(!p.busy, "verdict stops the poll");
        assert_eq!(p.poll_attempts, 0);
        assert_eq!(p.status.run_id.as_deref(), Some("r1"));
    }

    #[test]
    fn existing_run_does_not_auto_run() {
        let mut p = RoutingPanel::new();
        let status = parse_status(
            r#"{"run_id":"v-9","passed":true,"reachable":[],
            "failed":[],"missing_reporters":[]}"#,
        );
        let _ = p.update(Message::Loaded(Ok(status)));
        assert!(!p.auto_ran, "a real run is present — no auto-run");
        assert!(!p.busy);
    }

    #[test]
    fn view_renders_all_states_without_panic() {
        let mut p = RoutingPanel::new();
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view(); // empty
        p.status = parse_status(
            r#"{"run_id":"v-2","passed":false,"reachable":[],
               "failed":[{"from":"pine","to":"oak"}],"missing_reporters":["birch"]}"#,
        );
        p.run_result = Some(Ok("requested".into()));
        let _ = p.view(); // fail verdict + strip
    }

    // --- ROUTE-TRACE-4: trace toolbar state machine ----------------------------

    #[test]
    fn trace_egress_needs_a_source_node_to_be_traceable() {
        // Default direction is egress; a blank source can't trace; once a source
        // is set it can, and the request body is the exact egress shape.
        let mut t = TraceState::default();
        assert_eq!(t.dir(), Direction::Egress, "default direction is egress");
        assert!(!t.can_trace(), "blank egress source is not traceable");
        assert!(t.request_body().is_none());

        t.source = "eagle".into();
        t.dest = "1.1.1.1".into();
        assert!(t.can_trace());
        let body: serde_json::Value =
            serde_json::from_str(&t.request_body().expect("traceable")).unwrap();
        assert_eq!(body["direction"], "egress");
        assert_eq!(body["from"], "eagle");
        assert_eq!(body["to"], "1.1.1.1");
    }

    #[test]
    fn trace_ingress_needs_a_dest_service_and_drops_the_source() {
        // Ingress gates on the destination service id (the responder resolves the
        // host from the service policy), and the body carries no `from`.
        let mut t = TraceState {
            direction: "ingress".into(),
            source: "eagle".into(), // present but irrelevant for ingress
            ..Default::default()
        };
        assert_eq!(t.dir(), Direction::Ingress);
        assert!(!t.can_trace(), "blank ingress dest is not traceable");
        assert!(t.request_body().is_none());

        t.dest = "grafana".into();
        assert!(t.can_trace());
        let body: serde_json::Value =
            serde_json::from_str(&t.request_body().expect("traceable")).unwrap();
        assert_eq!(body["direction"], "ingress");
        assert_eq!(body["to"], "grafana");
        assert!(body.get("from").is_none(), "ingress carries no 'from'");
    }

    #[test]
    fn switching_direction_reuses_the_endpoints() {
        // The direction toggle flips which field gates Trace without clearing the
        // other — the same endpoints serve both perspectives (ROUTE-TRACE-4
        // "switches egress↔ingress for the same endpoints").
        let mut t = TraceState {
            source: "eagle".into(),
            dest: "grafana".into(),
            ..Default::default()
        };
        // Egress: traceable, egress body shape.
        assert_eq!(t.dir(), Direction::Egress);
        assert!(t.can_trace());
        // Flip to ingress: still traceable (dest is set), ingress body shape.
        t.direction = "ingress".into();
        assert_eq!(t.dir(), Direction::Ingress);
        assert!(t.can_trace());
        let body: serde_json::Value = serde_json::from_str(&t.request_body().unwrap()).unwrap();
        assert_eq!(body["direction"], "ingress");
        assert_eq!(body["to"], "grafana");
    }

    #[test]
    fn a_busy_trace_is_not_re_triggerable() {
        let t = TraceState {
            source: "eagle".into(),
            busy: true,
            ..Default::default()
        };
        assert!(!t.can_trace(), "an in-flight trace gates the button");
        assert!(t.request_body().is_none());
    }

    #[test]
    fn update_drives_the_toolbar_then_renders_the_graph() {
        // The toolbar messages mutate the state machine and TraceClicked only
        // fires when traceable; a returned PathGraph renders without panic.
        let mut p = RoutingPanel::new();
        let _ = p.update(Message::TraceSourceChanged("eagle".into()));
        let _ = p.update(Message::TraceDestChanged("1.1.1.1".into()));
        assert_eq!(p.trace.source, "eagle");
        assert!(p.trace.can_trace());
        // A blocked ingress graph (mesh-only service) renders the blocked path.
        let g =
            mackes_mesh_types::route_trace::assemble_egress("eagle", Some("10.42.0.2"), "1.1.1.1");
        let _ = p.update(Message::TraceLoaded(Ok(g)));
        assert!(p.trace.result.is_some());
        let _ = p.view(); // graph card reachable from the real view
    }

    #[test]
    fn blocked_edge_label_uses_human_node_labels() {
        // A blocked ingress trace to a mesh-only service: the verdict should read
        // the node labels (Internet → the lighthouse), not the raw wire edge id.
        let g = mackes_mesh_types::route_trace::assemble_ingress(
            &mackes_mesh_types::exposure::ExposurePolicy {
                id: "grafana".into(),
                source: mackes_mesh_types::exposure::ServiceSource {
                    node: "eagle".into(),
                    port: 3000,
                    proto: "tcp".into(),
                    ..Default::default()
                },
                tier: mackes_mesh_types::exposure::Tier::MeshOnly,
                ..Default::default()
            },
            Some("10.42.0.2"),
            None,
        );
        let at = g.blocked_at.as_deref().expect("mesh-only blocks");
        let label = blocked_edge_label(&g, at);
        // internet->ingress edge → "Internet → (no ingress)" (the labels), no
        // raw "internet->ingress" wire id.
        assert!(label.contains('→'), "{label}");
        assert!(label.starts_with("Internet"), "{label}");
        assert!(!label.contains("->"), "no raw wire id: {label}");
        // An unknown edge id falls back to the raw id.
        assert_eq!(blocked_edge_label(&g, "ghost->void"), "ghost->void");
    }

    #[test]
    fn parse_trace_reply_decodes_ok_and_error_envelopes() {
        // The ok envelope yields a PathGraph; the error envelope an Err.
        let g = mackes_mesh_types::route_trace::assemble_egress("eagle", None, "1.1.1.1");
        let ok = format!("{{\"ok\":true,\"graph\":{}}}", g.to_json().unwrap());
        let decoded = parse_trace_reply(&ok).expect("ok envelope decodes");
        assert_eq!(decoded.direction, Direction::Egress);
        assert_eq!(decoded.nodes.len(), 2);

        let err = parse_trace_reply(r#"{"error":"no such service 'nope'"}"#).unwrap_err();
        assert!(err.contains("no such service"));
        assert!(parse_trace_reply("garbage").is_err());
        assert!(
            parse_trace_reply(r#"{"ok":true}"#).is_err(),
            "missing graph"
        );
    }

    // --- ROUTE-TRACE-5: per-hop control-point list -----------------------------

    #[test]
    fn verdict_severity_maps_each_verdict_to_its_carbon_support_tone() {
        // The badge's tone derives from control.verdict via the shared
        // BadgeSeverity ramp — Allow=Success(green), Block=Danger(red),
        // Indeterminate=Warning(amber) — which status_badge tints from Carbon
        // tokens (never a raw hex). Pinning the derivation here makes the §4 tone
        // mapping verifiable without rendering.
        let cp = |verdict: Verdict| ControlPoint {
            firewall: "firewalld:public".into(),
            verdict,
            rule: "x".into(),
        };
        assert_eq!(
            verdict_severity(cp(Verdict::Allow).verdict),
            BadgeSeverity::Success,
            "Allow reads success (permitted)"
        );
        assert_eq!(
            verdict_severity(cp(Verdict::Block).verdict),
            BadgeSeverity::Danger,
            "Block reads danger (path stops here)"
        );
        assert_eq!(
            verdict_severity(cp(Verdict::Indeterminate).verdict),
            BadgeSeverity::Warning,
            "Indeterminate reads warning (unresolved, not guessed)"
        );
        // The three tones are distinct — a glanceable green/red/amber chip.
        assert_ne!(
            verdict_severity(Verdict::Allow),
            verdict_severity(Verdict::Block)
        );
        assert_ne!(
            verdict_severity(Verdict::Block),
            verdict_severity(Verdict::Indeterminate)
        );
    }

    #[test]
    fn verdict_label_is_a_short_uppercase_chip() {
        assert_eq!(verdict_label(Verdict::Allow), "ALLOW");
        assert_eq!(verdict_label(Verdict::Block), "BLOCK");
        assert_eq!(verdict_label(Verdict::Indeterminate), "INDET");
    }

    #[test]
    fn control_hops_list_renders_a_row_per_control_and_none_when_unconstrained() {
        // An ingress trace to a mesh-only service crosses the public boundary
        // control point (a Block), so the list renders at least one hop row and
        // the helpers don't panic for a real assembled graph. The blocking edge is
        // the one `blocked_at` cites.
        let palette = mde_theme::Palette::gray_90();
        let blocked = mackes_mesh_types::route_trace::assemble_ingress(
            &mackes_mesh_types::exposure::ExposurePolicy {
                id: "grafana".into(),
                source: mackes_mesh_types::exposure::ServiceSource {
                    node: "eagle".into(),
                    port: 3000,
                    proto: "tcp".into(),
                    ..Default::default()
                },
                tier: mackes_mesh_types::exposure::Tier::MeshOnly,
                ..Default::default()
            },
            Some("10.42.0.2"),
            None,
        );
        // At least one edge carries a control point ⇒ the list is rendered.
        assert!(blocked.edges.iter().any(|e| e.control.is_some()));
        assert!(
            control_hops_list(&blocked, palette).is_some(),
            "a constrained path renders the control list"
        );
        // The blocking edge resolves to a renderable row.
        let blocking = blocked
            .edges
            .iter()
            .find(|e| e.is_blocked())
            .expect("mesh-only blocks at the boundary");
        assert!(control_hop_row(&blocked, blocking, palette).is_some());
        let _ = firewall_badge(blocking.control.as_ref().unwrap(), palette);

        // A plain egress with no modeled firewall crosses no control points ⇒ the
        // list collapses to None (no empty chrome), and a no-control edge yields no
        // row.
        let open = mackes_mesh_types::route_trace::assemble_egress("eagle", None, "1.1.1.1");
        assert!(open.edges.iter().all(|e| e.control.is_none()));
        assert!(
            control_hops_list(&open, palette).is_none(),
            "an unconstrained path renders no control list"
        );
        assert!(control_hop_row(&open, &open.edges[0], palette).is_none());
    }
}
