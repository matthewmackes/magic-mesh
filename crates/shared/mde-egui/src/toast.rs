//! `toast` — the **KIRON** transient feedback state machine (governance §4/§6;
//! `docs/design/kiron-toast-pattern.md`, KIRON-1).
//!
//! "Kiron" = *chyron*, the TV-news lower-third. This is the ONE canonical
//! transient-alert surface for the platform: a news-style band that every surface
//! (chat, seat/host-controls OSD, security, build-farm, compute) emits into,
//! replacing the ad-hoc overlays each rolled on its own.
//!
//! This module is KIRON-**1** — the pure model + the state machine + the two egui
//! renders, living beside [`crate::Style`]/[`crate::Motion`]/[`crate::widgets`]:
//!
//! - [`Toast`] — the model any surface constructs (tier, source-host, flag,
//!   headline, optional action, dwell).
//! - [`ToastHost`] — the **pure, headless-testable** state machine: a one-at-a-time
//!   rotating alert queue with Critical-preempt + until-acknowledged hold, a
//!   "N more" backlog counter, hover-pause, and a *separate* replace-in-place OSD
//!   level channel. Time is **injected** ([`ToastHost::tick`] takes the elapsed
//!   delta) — the model never reads a wall clock, so it is unit-tested without a GPU
//!   or a clock.
//! - [`ToastHost::chyron`] / [`ToastHost::osd`] — the **HIG banner** renderer (a
//!   top-center drop-in card — WL-UX-006/U13, PLATFORM-INTERFACES Q14: banners
//!   ride this existing toast plumbing, presentation only; the queue/dwell state
//!   machine is untouched) plus the active Carbon OSD pill over `Style` +
//!   `Motion`.
//!
//! **Out of scope here (KIRON-2, shell-side):** the `event/toast/show` Bus lane, the
//! notification sound, DND / focus-mute suppression, and executing an action's verb.
//! The action carried on a [`Toast`] is an *opaque* label+verb pair this crate never
//! runs — [`ToastHost::chyron`] only reports the clicked verb back to the shell.

use std::collections::VecDeque;
use std::time::Duration;

use egui::{pos2, vec2, Align2, Color32, Context, FontFamily, FontId, Rect, Sense, Ui};

use crate::carbon::paint_carbon;
use crate::motion::Spring;
use crate::style::Elevation;
use crate::{Motion, Style};

/// Default on-screen dwell for an [`Severity::Info`] chyron — short, low-friction.
pub const DWELL_INFO: Duration = Duration::from_secs(4);
/// Default on-screen dwell for a [`Severity::Warning`] chyron — worth reading.
pub const DWELL_WARNING: Duration = Duration::from_secs(7);
/// Dwell for the centered OSD pill — a quick hardware-feedback flash.
pub const DWELL_OSD: Duration = Duration::from_millis(1500);

// Stable egui ids for the two floating areas + their motion animations. String
// keys (not style values), so they carry no palette/spacing meaning.
const CHYRON_AREA_ID: &str = "kiron-chyron-area";
const CHYRON_ANIM_ID: &str = "kiron-chyron-anim";
const CHYRON_HOVER_ID: &str = "kiron-chyron-hover";
const OSD_AREA_ID: &str = "kiron-osd-area";
const OSD_ANIM_ID: &str = "kiron-osd-anim";

/// Alert severity — the color + preempt axis for a chyron. Ordered least-severe
/// first, so a derived comparison reads "`Critical` is the greatest".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Informational (accent). The shortest dwell.
    Info,
    /// Worth noticing (amber). A longer dwell.
    Warning,
    /// Needs attention now (red). Preempts to the front and holds until
    /// [`ToastHost::acknowledge`].
    Critical,
}

impl Severity {
    /// The `Style` palette token this severity paints its flag + accent bar with.
    #[must_use]
    pub const fn color(self) -> Color32 {
        match self {
            Self::Info => Style::SUPPORT_INFO,
            Self::Warning => Style::SUPPORT_WARNING,
            Self::Critical => Style::SUPPORT_ERROR,
        }
    }

    /// The severity-scaled default [`Dwell`]: Info short, Warning longer, and a
    /// Critical holds [`Dwell::UntilAck`] (safety over immersion — lock 6).
    #[must_use]
    pub const fn dwell(self) -> Dwell {
        match self {
            Self::Info => Dwell::For(DWELL_INFO),
            Self::Warning => Dwell::For(DWELL_WARNING),
            Self::Critical => Dwell::UntilAck,
        }
    }

    /// The Mackes-Carbon glyph this severity's banner / notification row paints
    /// (WL-UX-006/U13 — PLATFORM-INTERFACES Q14). Every name resolves in the
    /// curated [`crate::carbon`] registry (asserted in the tests); the painters
    /// fall back to a plain severity dot if a name ever leaves it.
    #[must_use]
    pub const fn glyph_name(self) -> &'static str {
        match self {
            Self::Info => "notification",
            Self::Warning => "dialog-warning",
            Self::Critical => "process-stop",
        }
    }
}

/// What kind of hardware level the OSD bar is reporting — selects the glyph + tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsdKind {
    /// Audio output volume.
    Volume,
    /// Audio output muted.
    Muted,
    /// Display brightness.
    Brightness,
}

impl OsdKind {
    /// The short monospace glyph label painted beside the level bar. Intel One
    /// Mono carries these ASCII forms on every seat (no icon-font dependency).
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Volume => "VOL",
            Self::Muted => "MUT",
            Self::Brightness => "BRT",
        }
    }
}

/// A hardware level reading for the OSD tier: a [`OsdKind`] + a `0.0..=1.0` level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OsdLevel {
    /// Which hardware axis this reading is for.
    pub kind: OsdKind,
    /// The level, clamped to `0.0..=1.0` at render time.
    pub level: f32,
}

impl OsdLevel {
    /// A new level reading. `level` is stored as given and clamped when painted.
    #[must_use]
    pub const fn new(kind: OsdKind, level: f32) -> Self {
        Self { kind, level }
    }
}

/// The two toast families that share one host (lock 2).
///
/// Alert chyrons carry a [`Severity`]; the OSD tier carries a hardware
/// [`OsdLevel`] and renders separately, replace-in-place (never queued behind
/// alerts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tier {
    /// A lower-third alert chyron at the given severity.
    Alert(Severity),
    /// A centered hardware-level OSD (volume / brightness), replace-in-place.
    Osd(OsdLevel),
}

/// How long a [`Toast`] dwells on screen before the host advances past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dwell {
    /// Auto-advance after this much on-screen time (hover pauses the countdown).
    For(Duration),
    /// Never auto-advances — stays until [`ToastHost::acknowledge`] (Critical).
    UntilAck,
}

/// The optional click-through on a chyron: a button `label` plus an **opaque**
/// action `verb`.
///
/// KIRON-1 never executes the verb — [`ToastHost::chyron`] reports a clicked verb
/// back to the shell, which wires it to navigation in KIRON-2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastAction {
    /// The button caption ("Open" / "Go to" / …).
    pub label: String,
    /// The opaque action verb the shell resolves (e.g. `chat/open/<peer>`).
    pub verb: String,
}

/// A single toast any surface constructs and hands to a [`ToastHost`].
///
/// Reads like a TV news lower-third: a severity-colored category `flag`, the
/// originating `source_host` (hostname = user identity on the mesh), a one-line
/// `headline`, and an optional click-through `action`.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// Which family + severity/level this toast is.
    pub tier: Tier,
    /// The originating hostname (mesh identity). Empty for a local OSD flash.
    pub source_host: String,
    /// The category flag — `SECURITY` / `BUILD` / `CHAT` / … (the left chip).
    pub flag: String,
    /// The single-line headline shown in the band's center.
    pub headline: String,
    /// The optional click-through button (label + opaque verb).
    pub action: Option<ToastAction>,
    /// How long this toast dwells before the host advances (severity-scaled).
    pub dwell: Dwell,
}

impl Toast {
    /// A new alert chyron with the severity's default [`Dwell`] and no action.
    #[must_use]
    pub fn alert(
        severity: Severity,
        source_host: impl Into<String>,
        flag: impl Into<String>,
        headline: impl Into<String>,
    ) -> Self {
        Self {
            tier: Tier::Alert(severity),
            source_host: source_host.into(),
            flag: flag.into(),
            headline: headline.into(),
            action: None,
            dwell: severity.dwell(),
        }
    }

    /// Attach a click-through action (a button `label` + an opaque `verb`).
    #[must_use]
    pub fn with_action(mut self, label: impl Into<String>, verb: impl Into<String>) -> Self {
        self.action = Some(ToastAction {
            label: label.into(),
            verb: verb.into(),
        });
        self
    }

    /// Override the default dwell (e.g. to hold a Warning longer).
    #[must_use]
    pub const fn with_dwell(mut self, dwell: Dwell) -> Self {
        self.dwell = dwell;
        self
    }

    /// A hardware-level OSD toast (volume / brightness). Carries no host/flag/
    /// headline — it renders as the centered pill, not a chyron.
    #[must_use]
    pub const fn osd(level: OsdLevel) -> Self {
        Self {
            tier: Tier::Osd(level),
            source_host: String::new(),
            flag: String::new(),
            headline: String::new(),
            action: None,
            dwell: Dwell::For(DWELL_OSD),
        }
    }
}

/// The currently-showing alert plus its live countdown (`None` = held until ack).
#[derive(Debug, Clone)]
struct Active {
    toast: Toast,
    remaining: Option<Duration>,
}

impl Active {
    const fn new(toast: Toast) -> Self {
        let remaining = match toast.dwell {
            Dwell::For(d) => Some(d),
            Dwell::UntilAck => None,
        };
        Self { toast, remaining }
    }

    const fn is_critical(&self) -> bool {
        matches!(self.toast.tier, Tier::Alert(Severity::Critical))
    }
}

/// The active OSD flash + its (never-paused) countdown.
#[derive(Debug, Clone)]
struct ActiveOsd {
    toast: Toast,
    remaining: Duration,
}

/// What a chyron frame's widgets reported back — applied to the host after the
/// render closure returns (so the closure never has to borrow the host).
#[derive(Debug, Default, Clone)]
struct BandOutcome {
    hovered: bool,
    dismissed: bool,
    acknowledged: bool,
    action: Option<String>,
}

/// What [`ToastHost::chyron`] reports to the shell each frame: the opaque action
/// verb the user clicked, if any.
///
/// The shell (KIRON-2) resolves the verb to navigation; dismiss/acknowledge are
/// already applied to the host by the render.
#[derive(Debug, Default, Clone)]
pub struct ChyronInteraction {
    /// The clicked action verb, if the user pressed the chyron's action button.
    pub action: Option<String>,
}

/// The pure alert/OSD state machine the shell paints once per frame.
///
/// One host owns two channels: a **one-at-a-time rotating alert queue** (with
/// Critical-preempt + until-ack hold + a "N more" backlog) and a **separate
/// replace-in-place OSD level** channel. Every queue transition is a pure method;
/// time is injected via [`tick`](Self::tick).
#[derive(Debug, Default)]
pub struct ToastHost {
    /// The alert showing right now (its countdown lives here).
    current: Option<Active>,
    /// Alerts waiting their turn — the "N more" backlog.
    pending: VecDeque<Toast>,
    /// The separate OSD level flash (never queued behind alerts).
    osd: Option<ActiveOsd>,
    /// Whether the band is hovered — pauses the alert countdown.
    hovered: bool,
    /// The last alert painted, retained so it can ease out after it leaves.
    chyron_fade: Option<Toast>,
    /// The last OSD painted, retained for its ease-out.
    osd_fade: Option<Toast>,
}

impl ToastHost {
    /// A fresh, empty host.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current: None,
            pending: VecDeque::new(),
            osd: None,
            hovered: false,
            chyron_fade: None,
            osd_fade: None,
        }
    }

    // ── queue transitions (pure) ────────────────────────────────────────────

    /// Enqueue an alert. If nothing is showing it shows immediately; a **Critical**
    /// preempts a non-critical to the front (the displaced alert resumes after the
    /// Critical is acknowledged); otherwise it joins the back of the backlog.
    ///
    /// An OSD-tier toast passed here routes to [`flash_osd`](Self::flash_osd) — the
    /// OSD level never queues behind alerts.
    pub fn enqueue(&mut self, toast: Toast) {
        if let Tier::Osd(level) = toast.tier {
            self.flash_osd(level);
            return;
        }
        let incoming_critical = matches!(toast.tier, Tier::Alert(Severity::Critical));
        match &self.current {
            None => self.current = Some(Active::new(toast)),
            Some(cur) if incoming_critical && !cur.is_critical() => {
                if let Some(displaced) = self.current.take() {
                    self.pending.push_front(displaced.toast);
                }
                self.current = Some(Active::new(toast));
            }
            Some(_) => self.pending.push_back(toast),
        }
    }

    /// Flash the centered OSD pill, **replacing** any current one in
    /// place. Independent of the alert queue (a direct hardware-feedback path).
    pub fn flash_osd(&mut self, level: OsdLevel) {
        self.osd = Some(ActiveOsd {
            toast: Toast::osd(level),
            remaining: DWELL_OSD,
        });
    }

    /// Drop the showing alert and promote the next from the backlog (if any).
    pub fn advance(&mut self) {
        self.current = self.pending.pop_front().map(Active::new);
    }

    /// Dismiss the showing alert (a click / "X" / swipe). A **Critical** is *not*
    /// dismissable this way — it requires an explicit [`acknowledge`](Self::acknowledge).
    pub fn dismiss(&mut self) {
        match &self.current {
            // UntilAck (Critical) must be acknowledged, not dismissed.
            Some(active) if active.remaining.is_none() => {}
            Some(_) => self.advance(),
            None => {}
        }
    }

    /// Acknowledge the showing alert — the only way to clear a **Critical**. On a
    /// non-critical this is a no-op (use [`dismiss`](Self::dismiss)).
    pub fn acknowledge(&mut self) {
        if self.current.as_ref().is_some_and(Active::is_critical) {
            self.advance();
        }
    }

    /// Set whether the band is hovered — pauses the alert countdown while `true`.
    pub const fn set_hover(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Advance every countdown by the injected `elapsed` delta.
    ///
    /// The OSD flash always counts down (instant hardware feedback — never
    /// hover-paused); the alert countdown is paused while hovered and an
    /// [`Dwell::UntilAck`] Critical never expires. An alert whose countdown hits
    /// zero auto-advances.
    pub fn tick(&mut self, elapsed: Duration) {
        if let Some(osd) = &mut self.osd {
            osd.remaining = osd.remaining.saturating_sub(elapsed);
            if osd.remaining.is_zero() {
                self.osd = None;
            }
        }

        if self.hovered {
            return;
        }
        if let Some(active) = &mut self.current {
            if let Some(rem) = &mut active.remaining {
                *rem = rem.saturating_sub(elapsed);
                if rem.is_zero() {
                    self.advance();
                }
            }
        }
    }

    // ── read state ──────────────────────────────────────────────────────────

    /// The alert showing right now, if any.
    #[must_use]
    pub fn current(&self) -> Option<&Toast> {
        self.current.as_ref().map(|a| &a.toast)
    }

    /// The "N more" backlog count — alerts waiting behind the current one.
    #[must_use]
    pub fn backlog(&self) -> usize {
        self.pending.len()
    }

    /// Whether the showing alert is a Critical (held until acknowledged).
    #[must_use]
    pub fn has_critical(&self) -> bool {
        self.current.as_ref().is_some_and(Active::is_critical)
    }

    /// The showing alert's remaining dwell — `None` for an until-ack Critical or
    /// when nothing is showing.
    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        self.current.as_ref().and_then(|a| a.remaining)
    }

    /// Whether an OSD level bar is currently flashing.
    #[must_use]
    pub const fn osd_active(&self) -> bool {
        self.osd.is_some()
    }

    /// Whether nothing is showing or queued (both channels idle).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.current.is_none() && self.pending.is_empty() && self.osd.is_none()
    }

    // ── renders (over Style + Motion) ─────────────────────────────────────────

    /// Paint the top-center **HIG banner** card for the current alert (a spring
    /// drop in on [`Spring::SNAPPY`], a fade back out as the dwell expires —
    /// WL-UX-006/U13, PLATFORM-INTERFACES Q14) and return the clicked action
    /// verb, if any.
    ///
    /// Side effects applied to the host: hover-pause is fed from the card's hover
    /// state, a dismiss/acknowledge click is applied directly. The action verb is
    /// *reported* (KIRON-2 resolves it to navigation) — never executed here.
    pub fn chyron(&mut self, ctx: &Context) -> ChyronInteraction {
        let present = self.current.is_some();
        // The drop spring is seeded at 0 by every absent frame (this render runs
        // each frame), so a fresh alert springs down from above the screen edge
        // rather than popping in place.
        let t = Motion::spring_to(
            ctx,
            CHYRON_ANIM_ID,
            if present { 1.0 } else { 0.0 },
            Spring::SNAPPY,
        );

        // The toast to paint: the live one (also retained for its slide-out), or
        // the retained one while it eases away.
        let toast = if let Some(active) = &self.current {
            self.chyron_fade = Some(active.toast.clone());
            active.toast.clone()
        } else if t > BANNER_GONE {
            match &self.chyron_fade {
                Some(faded) => faded.clone(),
                None => return ChyronInteraction::default(),
            }
        } else {
            self.chyron_fade = None;
            return ChyronInteraction::default();
        };

        let backlog = self.pending.len();
        let remaining = self.remaining();
        let mut band = BandOutcome::default();
        egui::Area::new(egui::Id::new(CHYRON_AREA_ID))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                band = paint_banner(ui, &toast, backlog, remaining, t);
            });

        self.set_hover(band.hovered);
        if band.acknowledged {
            self.acknowledge();
        }
        if band.dismissed {
            self.dismiss();
        }
        ChyronInteraction {
            action: band.action,
        }
    }

    /// Paint the centered OSD pill for the current flash (a quick rise on
    /// [`Motion::FAST`]). Instant + interaction-free — no queue, no dismissal.
    pub fn osd(&mut self, ctx: &Context) {
        let present = self.osd.is_some();
        let t = Motion::animate(ctx, OSD_ANIM_ID, present, Motion::FAST);

        let tier = if let Some(osd) = &self.osd {
            self.osd_fade = Some(osd.toast.clone());
            osd.toast.tier
        } else if t > f32::EPSILON {
            match &self.osd_fade {
                Some(faded) => faded.tier,
                None => return,
            }
        } else {
            self.osd_fade = None;
            return;
        };

        let Tier::Osd(level) = tier else { return };
        egui::Area::new(egui::Id::new(OSD_AREA_ID))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                paint_osd(ui, level, t);
            });
    }
}

// ── the HIG banner card (WL-UX-006/U13 — PLATFORM-INTERFACES Q14) ────────────

/// The banner card's widest reading — narrower screens inset by [`Style::SP_L`].
const BANNER_MAX_W: f32 = 520.0;
/// The banner card height on the spacing ladder: a `TYPE_BODY` title line over a
/// `TYPE_FOOTNOTE` detail line plus padding.
const BANNER_H: f32 = Style::SP_XL + Style::SP_L;
/// Resting gap between the banner card and the screen's top edge.
const BANNER_MARGIN: f32 = Style::SP_M;
/// Below this drop progress the banner is treated as gone — aligned with the
/// [`Spring::settled`] epsilon, because an asymptoting spring never quite
/// reaches `0.0` the way the old fixed-duration ease did.
const BANNER_GONE: f32 = 0.02;
/// The severity glyph's square plate, on the spacing ladder.
const BANNER_GLYPH_PLATE: f32 = Style::SP_XL;

/// The banner card's rect at drop progress `t`: `0` parks it fully above the
/// screen, `1` rests it top-center ([`BANNER_MARGIN`] below the edge), and a
/// spring's overshoot past `1` reads as the drop's bounce.
fn banner_rect(screen: Rect, t: f32) -> Rect {
    let w = (screen.width() - 2.0 * Style::SP_L).clamp(0.0, BANNER_MAX_W);
    let x = screen.center().x - w / 2.0;
    let parked = screen.top() - BANNER_H - Style::SP_L;
    let resting = screen.top() + BANNER_MARGIN;
    let y = (resting - parked).mul_add(t, parked);
    Rect::from_min_size(pos2(x, y), vec2(w, BANNER_H))
}

/// The banner title face — [`Style::TYPE_BODY`], proportional (Q14 HIG type).
fn banner_title_font() -> FontId {
    FontId::new(Style::TYPE_BODY, FontFamily::Proportional)
}

/// The banner detail face — [`Style::TYPE_FOOTNOTE`], proportional.
fn banner_detail_font() -> FontId {
    FontId::new(Style::TYPE_FOOTNOTE, FontFamily::Proportional)
}

/// Paint the top-center HIG banner card and return what its widgets reported.
///
/// Presentation ONLY (U13): the queue, dedup, dwell, hover-pause, and
/// Critical-ack semantics all live in [`ToastHost`] untouched — this reads a
/// [`Toast`] and draws the Q14 banner (RADIUS_L card, Overlay elevation, Carbon
/// severity glyph, TYPE_BODY title + TYPE_FOOTNOTE detail).
fn paint_banner(
    ui: &mut Ui,
    toast: &Toast,
    backlog: usize,
    remaining: Option<Duration>,
    t: f32,
) -> BandOutcome {
    let Tier::Alert(severity) = toast.tier else {
        return BandOutcome::default();
    };
    let color = severity.color();
    let alpha = t.clamp(0.0, 1.0);

    let screen = ui.ctx().screen_rect();
    let card = banner_rect(screen, t);
    let cy = card.center().y;

    // Independent clone of the painter so the widget `put`s below can borrow `ui`.
    let painter = ui.painter().clone();
    // Elevation: the shared Overlay shadow, faded with the drop.
    let mut shadow = Elevation::Overlay.egui_shadow();
    shadow.color = shadow.color.gamma_multiply(alpha);
    painter.add(shadow.as_shape(card, Style::RADIUS_L));
    painter.rect_filled(card, Style::RADIUS_L, Style::SURFACE.gamma_multiply(alpha));
    painter.rect_stroke(
        card,
        Style::RADIUS_L,
        egui::Stroke::new(1.0, Style::BORDER.gamma_multiply(alpha)),
        egui::StrokeKind::Inside,
    );

    // Left: the Carbon severity glyph on a severity-tinted plate.
    let plate = Rect::from_center_size(
        pos2(card.left() + Style::SP_M + BANNER_GLYPH_PLATE / 2.0, cy),
        vec2(BANNER_GLYPH_PLATE, BANNER_GLYPH_PLATE),
    );
    painter.rect_filled(plate, Style::RADIUS_S, color.gamma_multiply(0.18 * alpha));
    if !paint_carbon(
        &painter,
        plate.shrink(Style::SP_XS),
        severity.glyph_name(),
        color.gamma_multiply(alpha),
    ) {
        // Registry miss: an honest severity dot rather than a blank plate.
        painter.circle_filled(plate.center(), Style::SP_XS, color.gamma_multiply(alpha));
    }

    // Center: the TYPE_BODY headline over a TYPE_FOOTNOTE `source · flag`
    // detail, clipped to the card (the old full-width band never truncated).
    let clipped = painter.with_clip_rect(card);
    let text_left = plate.right() + Style::SP_M;
    clipped.text(
        pos2(text_left, cy - Style::SP_XS / 2.0),
        Align2::LEFT_BOTTOM,
        &toast.headline,
        banner_title_font(),
        Style::TEXT.gamma_multiply(alpha),
    );
    let detail = match (toast.source_host.is_empty(), toast.flag.is_empty()) {
        (false, false) => format!("{} · {}", toast.source_host, toast.flag),
        (false, true) => toast.source_host.clone(),
        (true, false) => toast.flag.clone(),
        (true, true) => String::new(),
    };
    clipped.text(
        pos2(text_left, cy + Style::SP_XS / 2.0),
        Align2::LEFT_TOP,
        detail,
        banner_detail_font(),
        Style::TEXT_DIM.gamma_multiply(alpha),
    );

    // Right: dismiss/ack button, optional action button, then countdown + "N more".
    paint_banner_controls(ui, &painter, card, toast, backlog, remaining)
}

/// Paint the card's right-hand controls (dismiss/acknowledge + optional action +
/// countdown/"N more") and report the interaction, including card hover.
fn paint_banner_controls(
    ui: &mut Ui,
    painter: &egui::Painter,
    band: Rect,
    toast: &Toast,
    backlog: usize,
    remaining: Option<Duration>,
) -> BandOutcome {
    let critical = matches!(toast.tier, Tier::Alert(Severity::Critical));
    let cy = band.center().y;
    let btn_h = band.height() - Style::SP_M;
    let btn_w = Style::SP_XL * 2.6;
    let mut rx = band.right() - Style::SP_M;
    let mut out = BandOutcome::default();

    let (label, is_ack) = if critical {
        ("Acknowledge", true)
    } else {
        ("Dismiss", false)
    };
    let dz = Rect::from_min_max(
        pos2(rx - btn_w, cy - btn_h / 2.0),
        pos2(rx, cy + btn_h / 2.0),
    );
    let dismiss_resp = ui.put(
        dz,
        egui::Button::new(egui::RichText::new(label).size(Style::TYPE_FOOTNOTE)),
    );
    if dismiss_resp.clicked() {
        if is_ack {
            out.acknowledged = true;
        } else {
            out.dismissed = true;
        }
    }
    rx = dz.left() - Style::SP_S;

    if let Some(action) = &toast.action {
        let az = Rect::from_min_max(
            pos2(rx - btn_w, cy - btn_h / 2.0),
            pos2(rx, cy + btn_h / 2.0),
        );
        let action_resp = ui.put(
            az,
            egui::Button::new(
                egui::RichText::new(&action.label)
                    .size(Style::TYPE_FOOTNOTE)
                    .color(Style::BG),
            )
            .fill(Style::ACCENT),
        );
        if action_resp.clicked() {
            out.action = Some(action.verb.clone());
        }
        rx = az.left() - Style::SP_S;
    }

    let mut meta: Vec<String> = Vec::new();
    if let Some(rem) = remaining {
        meta.push(format!("{:.0}s", rem.as_secs_f32().ceil()));
    } else if critical {
        meta.push("HOLD".to_owned());
    }
    if backlog > 0 {
        meta.push(format!("{backlog} more"));
    }
    if !meta.is_empty() {
        // Monospace footnote keeps the counting-down digits stable-width.
        painter.text(
            pos2(rx - Style::SP_S, cy),
            Align2::RIGHT_CENTER,
            meta.join("  ·  "),
            FontId::new(Style::TYPE_FOOTNOTE, FontFamily::Monospace),
            Style::TEXT_DIM,
        );
    }

    // Hover over the whole band pauses the countdown.
    let hover = ui.interact(band, egui::Id::new(CHYRON_HOVER_ID), Sense::hover());
    out.hovered = hover.hovered() || dismiss_resp.hovered();
    out
}

/// Paint the centered Carbon OSD pill (glyph + level track + percent).
fn paint_osd(ui: &Ui, level: OsdLevel, t: f32) {
    let screen = ui.ctx().screen_rect();
    let width = Style::SP_XL * 7.5;
    let height = Style::SP_XL * 1.35;
    let rise = (1.0 - t) * Style::SP_M;
    let center = pos2(screen.center().x, screen.center().y + rise);
    let rect = Rect::from_center_size(center, vec2(width, height));

    let painter = ui.painter().clone();
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.text(
        pos2(rect.left() + Style::SP_M, rect.center().y),
        Align2::LEFT_CENTER,
        level.kind.glyph(),
        FontId::new(Style::SMALL, FontFamily::Monospace),
        Style::TEXT,
    );

    let track = Rect::from_min_max(
        pos2(
            rect.left() + Style::SP_XL + Style::SP_L,
            rect.center().y - Style::SP_XS / 2.0,
        ),
        pos2(
            rect.right() - Style::SP_XL - Style::SP_M,
            rect.center().y + Style::SP_XS / 2.0,
        ),
    );
    painter.rect_filled(track, Style::RADIUS, Style::BORDER);

    let fraction = level.level.clamp(0.0, 1.0);
    let fill = Rect::from_min_max(
        track.min,
        pos2(
            track.width().mul_add(fraction, track.left()),
            track.bottom(),
        ),
    );
    let fill_color = if matches!(level.kind, OsdKind::Muted) {
        Style::TEXT_DIM
    } else {
        Style::ACCENT
    };
    painter.rect_filled(fill, Style::RADIUS, fill_color);
    painter.text(
        pos2(rect.right() - Style::SP_M, rect.center().y),
        Align2::RIGHT_CENTER,
        format!("{:.0}%", fraction * 100.0),
        FontId::new(Style::SMALL, FontFamily::Monospace),
        Style::TEXT,
    );
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn info(host: &str) -> Toast {
        Toast::alert(Severity::Info, host, "CHAT", "a message arrived")
    }

    fn crit(host: &str) -> Toast {
        Toast::alert(Severity::Critical, host, "SECURITY", "intrusion detected")
    }

    // ── model ────────────────────────────────────────────────────────────────

    #[test]
    fn severity_dwell_scales_and_critical_holds() {
        assert!(matches!(Severity::Info.dwell(), Dwell::For(DWELL_INFO)));
        assert!(matches!(
            Severity::Warning.dwell(),
            Dwell::For(DWELL_WARNING)
        ));
        assert!(matches!(Severity::Critical.dwell(), Dwell::UntilAck));
        // Info dwells shorter than Warning; Critical is greatest severity.
        assert!(DWELL_INFO < DWELL_WARNING);
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Critical);
    }

    #[test]
    fn severity_colors_use_carbon_support_tokens() {
        assert_eq!(Severity::Info.color(), Style::SUPPORT_INFO);
        assert_eq!(Severity::Warning.color(), Style::SUPPORT_WARNING);
        assert_eq!(Severity::Critical.color(), Style::SUPPORT_ERROR);
    }

    // ── queue: enqueue / show / advance ───────────────────────────────────────

    #[test]
    fn enqueue_shows_first_immediately() {
        let mut host = ToastHost::new();
        assert!(host.is_idle());
        host.enqueue(info("nyc3"));
        assert_eq!(host.current().map(|t| t.source_host.as_str()), Some("nyc3"));
        assert_eq!(host.backlog(), 0);
        assert!(!host.is_idle());
    }

    #[test]
    fn backlog_counts_pending_and_advances_in_order() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.enqueue(info("b"));
        host.enqueue(info("c"));
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("a".into())
        );
        assert_eq!(host.backlog(), 2);
        host.advance();
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("b".into())
        );
        assert_eq!(host.backlog(), 1);
        host.advance();
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("c".into())
        );
        host.advance();
        assert!(host.current().is_none());
    }

    #[test]
    fn expiry_auto_advances_the_queue() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.enqueue(info("b"));
        host.tick(DWELL_INFO); // exactly drains the first's dwell
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("b".into())
        );
        assert_eq!(host.backlog(), 0);
        host.tick(DWELL_INFO + Duration::from_secs(1)); // over-tick past the last
        assert!(host.current().is_none());
    }

    // ── queue: Critical preempt + until-ack hold ──────────────────────────────

    #[test]
    fn critical_preempts_to_front_and_displaced_resumes() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.enqueue(crit("lh1"));
        // Critical jumped ahead; the info was pushed back into the backlog.
        assert!(host.has_critical());
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("lh1".into())
        );
        assert_eq!(host.backlog(), 1);
        host.acknowledge();
        // The displaced info resumes.
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("a".into())
        );
        assert!(!host.has_critical());
    }

    #[test]
    fn critical_holds_until_ack_and_ignores_tick_and_dismiss() {
        let mut host = ToastHost::new();
        host.enqueue(crit("lh1"));
        assert_eq!(host.remaining(), None); // UntilAck: no countdown
        host.tick(Duration::from_secs(3600));
        assert!(host.has_critical()); // still up after a huge tick
        host.dismiss();
        assert!(host.has_critical()); // dismiss can't clear a Critical
        host.acknowledge();
        assert!(host.current().is_none()); // only ack clears it
    }

    #[test]
    fn second_critical_appends_not_preempts() {
        let mut host = ToastHost::new();
        host.enqueue(crit("lh1"));
        host.enqueue(crit("lh2"));
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("lh1".into())
        );
        assert_eq!(host.backlog(), 1);
    }

    #[test]
    fn non_critical_does_not_preempt() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.enqueue(Toast::alert(
            Severity::Warning,
            "b",
            "BUILD",
            "build failed",
        ));
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("a".into())
        );
        assert_eq!(host.backlog(), 1);
    }

    // ── queue: dismiss + hover-pause ──────────────────────────────────────────

    #[test]
    fn dismiss_advances_a_non_critical() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.enqueue(info("b"));
        host.dismiss();
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("b".into())
        );
    }

    #[test]
    fn hover_pauses_the_countdown() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.set_hover(true);
        host.tick(DWELL_INFO * 2); // way past the dwell, but paused
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("a".into())
        );
        host.set_hover(false);
        host.tick(DWELL_INFO);
        assert!(host.current().is_none()); // resumes + expires
    }

    #[test]
    fn acknowledge_is_a_noop_on_non_critical() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.acknowledge();
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("a".into())
        );
    }

    // ── OSD tier: separate, replace-in-place, never queued ────────────────────

    #[test]
    fn osd_replaces_in_place_and_is_independent_of_alerts() {
        let mut host = ToastHost::new();
        host.enqueue(crit("lh1"));
        host.flash_osd(OsdLevel::new(OsdKind::Volume, 0.3));
        host.flash_osd(OsdLevel::new(OsdKind::Volume, 0.6)); // replaces, not queues
        assert!(host.osd_active());
        assert_eq!(host.backlog(), 0); // OSD never touched the alert backlog
        assert!(host.has_critical()); // alert untouched
    }

    #[test]
    fn osd_expires_on_tick_without_disturbing_alerts() {
        let mut host = ToastHost::new();
        host.enqueue(info("a"));
        host.flash_osd(OsdLevel::new(OsdKind::Brightness, 0.5));
        host.tick(DWELL_OSD);
        assert!(!host.osd_active()); // OSD flashed and faded
        assert_eq!(
            host.current().map(|t| t.source_host.clone()),
            Some("a".into())
        );
    }

    #[test]
    fn enqueue_osd_toast_routes_to_the_osd_channel() {
        let mut host = ToastHost::new();
        host.enqueue(Toast::osd(OsdLevel::new(OsdKind::Volume, 0.9)));
        assert!(host.osd_active());
        assert!(host.current().is_none()); // did NOT join the alert queue
    }

    // ── builders ──────────────────────────────────────────────────────────────

    #[test]
    fn with_action_carries_an_opaque_label_and_verb() {
        let toast = info("a").with_action("Open", "chat/open/a");
        let action = toast.action.expect("action set");
        assert_eq!(action.label, "Open");
        assert_eq!(action.verb, "chat/open/a");
    }

    // ── renders (headless tessellate) ─────────────────────────────────────────

    fn headless_ctx() -> Context {
        let ctx = Context::default();
        Style::install(&ctx);
        ctx
    }

    fn frame(ctx: &Context, mut body: impl FnMut(&Context)) -> Vec<egui::ClippedPrimitive> {
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 720.0))),
            ..Default::default()
        };
        // egui lays a brand-new floating `Area` out invisibly on its first frame
        // (it has no size yet) and repaints; the shell paints every frame, so warm
        // one frame then tessellate the second — that's the real steady-state paint.
        let _ = ctx.run(input(), |ctx| body(ctx));
        let output = ctx.run(input(), |ctx| body(ctx));
        ctx.tessellate(output.shapes, output.pixels_per_point)
    }

    #[test]
    fn chyron_tessellates_a_real_band_when_present() {
        let ctx = headless_ctx();
        let mut host = ToastHost::new();
        host.enqueue(info("nyc3").with_action("Open", "chat/open/nyc3"));

        let prims = frame(&ctx, |ctx| {
            let out = host.chyron(ctx);
            // No verb was clicked in a headless frame — nothing is executed here.
            assert!(out.action.is_none());
        });
        assert!(
            !prims.is_empty(),
            "the chyron produced no geometry when an alert was present"
        );

        // A fully-idle host, first frame: the band is absent — no geometry.
        let mut empty = ToastHost::new();
        let none = frame(&ctx, |ctx| {
            let _ = empty.chyron(ctx);
        });
        assert!(none.is_empty(), "an idle chyron still drew geometry");
    }

    #[test]
    fn osd_tessellates_a_real_centered_pill_when_present() {
        let ctx = headless_ctx();
        let mut host = ToastHost::new();
        host.flash_osd(OsdLevel::new(OsdKind::Volume, 0.65));

        let prims = frame(&ctx, |ctx| host.osd(ctx));
        assert!(
            !prims.is_empty(),
            "the centered OSD pill produced no geometry when flashing"
        );
    }

    // ── the HIG banner presentation (WL-UX-006/U13 — PLATFORM-INTERFACES Q14) ─

    #[test]
    fn banner_severity_glyphs_resolve_in_the_carbon_registry() {
        for severity in [Severity::Info, Severity::Warning, Severity::Critical] {
            assert!(
                crate::carbon::carbon_svg_bytes(severity.glyph_name()).is_some(),
                "{severity:?} banner glyph '{}' left the curated Carbon registry",
                severity.glyph_name(),
            );
        }
    }

    #[test]
    fn banner_rests_top_center_and_parks_above_the_screen() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 720.0));
        let rest = banner_rect(screen, 1.0);
        assert!(
            (rest.center().x - screen.center().x).abs() < 0.5,
            "the banner rests top-CENTER"
        );
        assert!((rest.top() - (screen.top() + BANNER_MARGIN)).abs() < f32::EPSILON);
        assert!(rest.width() <= BANNER_MAX_W);
        let parked = banner_rect(screen, 0.0);
        assert!(
            parked.bottom() <= screen.top(),
            "t = 0 parks the card fully above the screen for the drop-in"
        );
        // A spring's overshoot past 1 reads as the bounce: below resting.
        assert!(banner_rect(screen, 1.1).top() > rest.top());
        // A narrow screen insets rather than overflowing.
        let narrow = Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0));
        assert!(banner_rect(narrow, 1.0).width() <= narrow.width() - 2.0 * Style::SP_L);
    }

    #[test]
    fn banner_type_reads_body_over_footnote() {
        // Q14: the HIG type ladder — TYPE_BODY title over TYPE_FOOTNOTE detail.
        assert_eq!(banner_title_font().size, Style::TYPE_BODY);
        assert_eq!(banner_detail_font().size, Style::TYPE_FOOTNOTE);
    }
}
