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
//!   "N more" backlog counter, five-second transient dwell, hover-pause, and a
//!   *separate* replace-in-place OSD
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

use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Align, Align2, Color32, Context, FontId, Rect, Sense, Ui};

use crate::carbon::paint_carbon;
use crate::motion::Spring;
use crate::style::{Elevation, TypographyRole};
use crate::{Motion, Style};

/// Default on-screen dwell for an [`Severity::Info`] chyron.
pub const DWELL_INFO: Duration = Duration::from_secs(5);
/// Default on-screen dwell for a [`Severity::Warning`] chyron.
pub const DWELL_WARNING: Duration = Duration::from_secs(5);
/// Dwell for the centered OSD pill — a quick hardware-feedback flash.
pub const DWELL_OSD: Duration = Duration::from_millis(1500);

/// Maximum alerts retained behind the visible lower third.
///
/// Health grade F holds until acknowledgement, so a failed or hostile producer
/// must not grow the shell process without bound while the operator is away.
/// The visible alert is additional to this bounded backlog.
pub const MAX_ALERT_BACKLOG: usize = 64;

/// Maximum health authorities whose admitted generation is retained.
///
/// A health authority is one `(node, condition)` pair. Keeping this cache
/// bounded prevents a hostile publisher from growing the shell indefinitely.
/// Once full, an unseen authority fails closed so every admitted authority's
/// watermark remains valid for this [`ToastHost`]'s lifetime.
const MAX_HEALTH_AUTHORITY_WATERMARKS: usize = 256;

/// Stable flag for operator-originated AI deployment notices. These alerts use
/// the centered, red, constrained presentation instead of the ambient banner.
pub const AI_GENERATED_ALERT_FLAG: &str = "AI-GENERATED-ALERT";

// Stable egui ids for the two floating areas + their motion animations. String
// keys (not style values), so they carry no palette/spacing meaning.
const CHYRON_AREA_ID: &str = "kiron-chyron-area";
const CHYRON_ANIM_ID: &str = "kiron-chyron-anim";
const CHYRON_HOVER_ID: &str = "kiron-chyron-hover";
const OSD_AREA_ID: &str = "kiron-osd-area";
const OSD_ANIM_ID: &str = "kiron-osd-anim";

/// AI deployment warnings must remain visible while the lock curtain owns the
/// normal foreground layer.  Tooltip is egui's top presentation order (also
/// used by the direct-DRM software cursor), so the constrained operator card
/// stays above the curtain without promoting ordinary notifications.
fn chyron_order(toast: &Toast) -> egui::Order {
    if toast.is_ai_generated_alert() {
        egui::Order::Tooltip
    } else {
        egui::Order::Foreground
    }
}

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

    /// The default [`Dwell`]: non-critical messages remain for five seconds and
    /// a Critical holds [`Dwell::UntilAck`] (safety over immersion — lock 6).
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

/// Queue authority retained from one validated health snapshot.
///
/// `ToastHost` does not interpret health grades; it uses this identity only to
/// prevent an older snapshot for the same node/condition from rolling back an
/// already-admitted lower third.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HealthToastAuthority {
    condition_id: String,
    snapshot_generation: u64,
}

/// Highest health snapshot admitted for one node/condition authority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HealthAuthorityWatermark {
    source_host: String,
    condition_id: String,
    snapshot_generation: u64,
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
    /// How long this toast dwells before the host advances.
    pub dwell: Dwell,
    /// Health snapshot identity, when this toast came from the governed health
    /// projection rather than the generic notification lane.
    health_authority: Option<HealthToastAuthority>,
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
            health_authority: None,
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

    /// Whether this alert requests the dedicated centered operator presentation.
    /// Matching is ASCII-case-insensitive so wire producers cannot accidentally
    /// lose the safety treatment through capitalization drift.
    #[must_use]
    pub fn is_ai_generated_alert(&self) -> bool {
        self.flag.eq_ignore_ascii_case(AI_GENERATED_ALERT_FLAG)
    }

    /// Override the default dwell (e.g. to hold a Warning longer).
    #[must_use]
    pub const fn with_dwell(mut self, dwell: Dwell) -> Self {
        self.dwell = dwell;
        self
    }

    /// Bind a governed health lower third to its condition and snapshot.
    ///
    /// The node identity is already carried in [`Self::source_host`]. A zero
    /// generation or empty condition is retained as invalid input and rejected
    /// by [`ToastHost::enqueue`], keeping construction ergonomic while making
    /// admission fail closed.
    #[must_use]
    pub fn with_health_authority(
        mut self,
        condition_id: impl Into<String>,
        snapshot_generation: u64,
    ) -> Self {
        self.health_authority = Some(HealthToastAuthority {
            condition_id: condition_id.into(),
            snapshot_generation,
        });
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
            health_authority: None,
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

    const fn requires_acknowledgement(&self) -> bool {
        matches!(self.toast.dwell, Dwell::UntilAck)
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
    /// Highest admitted generation per health authority.
    /// Unlike `current`/`pending`, clearing a toast does not clear this state.
    health_watermarks: VecDeque<HealthAuthorityWatermark>,
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
            health_watermarks: VecDeque::new(),
        }
    }

    // ── queue transitions (pure) ────────────────────────────────────────────

    /// Retain one waiting alert without exceeding [`MAX_ALERT_BACKLOG`].
    ///
    /// Once saturated, a Critical may evict the newest non-critical waiter. A
    /// non-critical, or a Critical facing an all-Critical backlog, is rejected:
    /// already-admitted acknowledgement-required health alerts remain the
    /// fail-closed source of truth.
    fn retain_pending(&mut self, toast: Toast, front: bool) -> Result<(), Toast> {
        if self.pending.len() == MAX_ALERT_BACKLOG {
            let incoming_critical = matches!(toast.tier, Tier::Alert(Severity::Critical));
            if !incoming_critical {
                return Err(toast);
            }
            let Some(index) = self
                .pending
                .iter()
                .rposition(|pending| !matches!(pending.tier, Tier::Alert(Severity::Critical)))
            else {
                return Err(toast);
            };
            self.pending.remove(index);
        }

        if front {
            self.pending.push_front(toast);
        } else {
            self.pending.push_back(toast);
        }
        Ok(())
    }

    /// Admit only a forward health snapshot for one node/condition.
    ///
    /// Equal generations are coalesced as replays (including conflicting
    /// bodies), lower generations are rejected, and a forward generation
    /// replaces the older queued/current projection. This comparison happens
    /// before severity preemption, so stale recovery cannot dismiss or
    /// downgrade a visible grade E/F alert.
    fn coalesce_health(&mut self, toast: &Toast) -> bool {
        let Some(incoming) = &toast.health_authority else {
            return true;
        };
        if incoming.snapshot_generation == 0 || incoming.condition_id.is_empty() {
            return false;
        }

        let same_authority = |candidate: &Toast| {
            candidate.source_host == toast.source_host
                && candidate
                    .health_authority
                    .as_ref()
                    .is_some_and(|authority| authority.condition_id == incoming.condition_id)
        };
        if let Some(watermark) = self.health_watermarks.iter_mut().find(|watermark| {
            watermark.source_host == toast.source_host
                && watermark.condition_id == incoming.condition_id
        }) {
            if watermark.snapshot_generation >= incoming.snapshot_generation {
                return false;
            }
            watermark.snapshot_generation = incoming.snapshot_generation;
        } else {
            if self.health_watermarks.len() == MAX_HEALTH_AUTHORITY_WATERMARKS {
                return false;
            }
            self.health_watermarks.push_back(HealthAuthorityWatermark {
                source_host: toast.source_host.clone(),
                condition_id: incoming.condition_id.clone(),
                snapshot_generation: incoming.snapshot_generation,
            });
        }

        self.pending.retain(|candidate| !same_authority(candidate));
        if self
            .current
            .as_ref()
            .is_some_and(|active| same_authority(&active.toast))
        {
            self.current = Some(Active::new(toast.clone()));
            return false;
        }
        true
    }

    fn health_watermark_before(&self, toast: &Toast) -> Option<u64> {
        let authority = toast.health_authority.as_ref()?;
        self.health_watermarks
            .iter()
            .find(|watermark| {
                watermark.source_host == toast.source_host
                    && watermark.condition_id == authority.condition_id
            })
            .map(|watermark| watermark.snapshot_generation)
    }

    /// Undo the provisional watermark update when queue admission itself fails.
    /// A generation is admitted only when its toast is current or retained.
    fn restore_health_watermark(&mut self, toast: &Toast, previous: Option<u64>) {
        let Some(authority) = toast.health_authority.as_ref() else {
            return;
        };
        let index = self.health_watermarks.iter().position(|watermark| {
            watermark.source_host == toast.source_host
                && watermark.condition_id == authority.condition_id
        });
        match (index, previous) {
            (Some(index), Some(generation)) => {
                self.health_watermarks[index].snapshot_generation = generation;
            }
            (Some(index), None) => {
                self.health_watermarks.remove(index);
            }
            _ => {}
        }
    }

    /// Enqueue an alert. If nothing is showing it shows immediately; a **Critical**
    /// preempts a non-critical to the front (the displaced alert resumes after the
    /// Critical is acknowledged). An AI operator notice preempts every current alert,
    /// including a persistent Critical, so the mandatory update warning is actually
    /// visible during its five-second safety window; the displaced alert resumes from
    /// the front of the backlog afterward.
    /// Otherwise, the incoming alert joins the back of the backlog.
    ///
    /// An OSD-tier toast passed here routes to [`flash_osd`](Self::flash_osd) — the
    /// OSD level never queues behind alerts.
    pub fn enqueue(&mut self, toast: Toast) {
        if let Tier::Osd(level) = toast.tier {
            self.flash_osd(level);
            return;
        }
        let previous_health_watermark = self.health_watermark_before(&toast);
        if !self.coalesce_health(&toast) {
            return;
        }
        let incoming_critical = matches!(toast.tier, Tier::Alert(Severity::Critical));
        let incoming_operator_notice = toast.is_ai_generated_alert();
        match &self.current {
            None => self.current = Some(Active::new(toast)),
            Some(cur)
                if incoming_operator_notice
                    || (incoming_critical
                        && !cur.is_critical()
                        && !cur.toast.is_ai_generated_alert()) =>
            {
                if let Some(displaced) = self.current.take() {
                    let displaced_critical = displaced.is_critical();
                    if let Err(displaced_toast) = self.retain_pending(displaced.toast, true) {
                        if displaced_critical {
                            // A full acknowledgement-required backlog must not lose
                            // its visible Critical merely to show a newer alert.
                            self.current = Some(Active::new(displaced_toast));
                            if let Err(unadmitted) = self.retain_pending(toast, false) {
                                self.restore_health_watermark(
                                    &unadmitted,
                                    previous_health_watermark,
                                );
                            }
                            return;
                        }
                    }
                }
                self.current = Some(Active::new(toast));
            }
            Some(_) => {
                if let Err(unadmitted) = self.retain_pending(toast, false) {
                    self.restore_health_watermark(&unadmitted, previous_health_watermark);
                }
            }
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

    /// Acknowledge the showing alert — the only way to clear an
    /// [`Dwell::UntilAck`] alert. Timed Critical alerts (health grade E) retain
    /// their governed countdown and use [`dismiss`](Self::dismiss); only held
    /// Critical alerts (health grade F) enter this acknowledgement path.
    pub fn acknowledge(&mut self) {
        if self
            .current
            .as_ref()
            .is_some_and(Active::requires_acknowledgement)
        {
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
        let mut alert_elapsed = elapsed;
        loop {
            let Some(active) = &mut self.current else {
                break;
            };
            let Some(remaining) = &mut active.remaining else {
                break;
            };
            if alert_elapsed < *remaining {
                *remaining -= alert_elapsed;
                break;
            }
            alert_elapsed = alert_elapsed.saturating_sub(*remaining);
            self.advance();
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

    /// Whether the showing alert uses Critical presentation/preemption. Its
    /// dwell still decides whether it is timed (health E) or held (health F).
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

    /// Paint the current alert as either the top-center **HIG banner** or the
    /// centered red operator alert selected by [`AI_GENERATED_ALERT_FLAG`] (a
    /// spring in on [`Spring::SNAPPY`], a fade back out as the dwell expires —
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
            .order(chyron_order(&toast))
            .show(ctx, |ui| {
                band = if toast.is_ai_generated_alert() {
                    paint_ai_generated_alert(ui, &toast, backlog, remaining, t)
                } else {
                    paint_banner(ui, &toast, backlog, remaining, t)
                };
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
const BANNER_MAX_W: f32 = 900.0;
/// The banner card height on the spacing ladder: a `TYPE_BODY` title line over a
/// `TYPE_FOOTNOTE` detail line plus padding.
const BANNER_H: f32 = Style::SP_XL + Style::SP_L;
/// Narrow banners stack their command row below the message instead of letting
/// buttons consume or overlap the text lane.
const BANNER_STACKED_H: f32 = BANNER_H * 2.0;
const BANNER_STACKED_BREAKPOINT: f32 = 640.0;
/// Resting gap between the banner card and the screen's top edge.
const BANNER_MARGIN: f32 = Style::SP_M;
/// Below this drop progress the banner is treated as gone — aligned with the
/// [`Spring::settled`] epsilon, because an asymptoting spring never quite
/// reaches `0.0` the way the old fixed-duration ease did.
const BANNER_GONE: f32 = 0.02;
/// The severity glyph's square plate, on the spacing ladder.
const BANNER_GLYPH_PLATE: f32 = Style::SP_XL;
/// Width of each banner command. Keeping this in the geometry contract lets the
/// text lane stop before controls instead of painting beneath them.
const BANNER_BUTTON_W: f32 = Style::SP_XL * 2.6;
/// Reserved width for countdown and backlog metadata.
const BANNER_META_W: f32 = Style::SP_XL * 3.25;

/// The AI-generated deployment alert is intentionally compact: a readable
/// desktop modal, not a full-screen takeover or an unconstrained message panel.
const AI_ALERT_MAX_W: f32 = 680.0;
const AI_ALERT_H: f32 = 224.0;
const AI_ALERT_MARGIN: f32 = Style::SP_L;
const AI_ALERT_BUTTON_W: f32 = 132.0;
const AI_ALERT_BUTTON_H: f32 = 36.0;
const AI_ALERT_BLOCKER_ID: &str = "kiron-ai-alert-blocker";

/// The banner card's rect at drop progress `t`: `0` parks it fully above the
/// screen, `1` rests it top-center ([`BANNER_MARGIN`] below the edge), and a
/// spring's overshoot past `1` reads as the drop's bounce.
fn banner_rect(screen: Rect, t: f32) -> Rect {
    let w = 2.0f32
        .mul_add(-Style::SP_L, screen.width())
        .clamp(0.0, BANNER_MAX_W);
    let h = if w < BANNER_STACKED_BREAKPOINT {
        BANNER_STACKED_H
    } else {
        BANNER_H
    };
    let x = screen.center().x - w / 2.0;
    let parked = screen.top() - h - Style::SP_L;
    let resting = screen.top() + BANNER_MARGIN;
    let y = (resting - parked).mul_add(t, parked);
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

fn banner_is_stacked(card: Rect) -> bool {
    card.width() < BANNER_STACKED_BREAKPOINT
}

fn banner_control_rect(card: Rect) -> Rect {
    if banner_is_stacked(card) {
        Rect::from_min_max(pos2(card.left(), card.top() + BANNER_H), card.max)
    } else {
        card
    }
}

/// Right edge of the banner's dedicated text lane. The countdown/backlog and
/// buttons own everything to its right, so even an unusually long headline can
/// only truncate — it cannot overlap a control.
fn banner_text_right(card: Rect, has_action: bool) -> f32 {
    let button_count = if has_action { 2.0 } else { 1.0 };
    let button_gaps = if has_action { Style::SP_S } else { 0.0 };
    BANNER_BUTTON_W.mul_add(-button_count, card.right() - Style::SP_M)
        - button_gaps
        - Style::SP_S
        - BANNER_META_W
        - Style::SP_M
}

/// Centered constrained alert geometry. Motion is a restrained scale/fade from
/// 96% to full size; the card never changes center or exceeds the safe margins.
fn ai_alert_rect(screen: Rect, t: f32) -> Rect {
    let available_w = 2.0f32.mul_add(-AI_ALERT_MARGIN, screen.width()).max(1.0);
    let available_h = 2.0f32.mul_add(-AI_ALERT_MARGIN, screen.height()).max(1.0);
    let base = vec2(AI_ALERT_MAX_W.min(available_w), AI_ALERT_H.min(available_h));
    let scale = 0.04f32.mul_add(t.clamp(0.0, 1.0), 0.96);
    Rect::from_center_size(screen.center(), base * scale)
}

/// Translate a laid-out galley so its own bounds, including center-aligned
/// negative coordinates, are centered inside the alert card.
fn centered_galley_origin(card: Rect, top: f32, galley_rect: Rect) -> egui::Pos2 {
    pos2(
        card.center().x - galley_rect.center().x,
        top - galley_rect.top(),
    )
}

/// The banner title face — the shared [`TypographyRole::Body`] role (Q14 HIG
/// type).
fn banner_title_font() -> FontId {
    Style::typography_font(TypographyRole::Body)
}

/// The banner detail face — the shared [`TypographyRole::Label`] role.
fn banner_detail_font() -> FontId {
    Style::typography_font(TypographyRole::Label)
}

/// The lower-third close control follows the alert's lifecycle authority, not
/// its visual severity. Health grades E and F are both Critical (and therefore
/// both preempt), but only F carries [`Dwell::UntilAck`].
const fn close_control(toast: &Toast) -> (&'static str, bool) {
    if matches!(toast.dwell, Dwell::UntilAck) {
        ("Acknowledge", true)
    } else {
        ("Dismiss", false)
    }
}

/// Paint the top-center HIG banner card and return what its widgets reported.
///
/// Presentation ONLY (U13): the queue, dedup, dwell, hover-pause, and
/// Critical-ack semantics all live in [`ToastHost`] untouched — this reads a
/// [`Toast`] and draws the Q14 banner (`RADIUS_L` card, Overlay elevation, Carbon
/// severity glyph, `TYPE_BODY` title + `TYPE_FOOTNOTE` detail).
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
    let stacked = banner_is_stacked(card);
    let message_cy = if stacked {
        card.top() + BANNER_H / 2.0
    } else {
        card.center().y
    };

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
        pos2(
            card.left() + Style::SP_M + BANNER_GLYPH_PLATE / 2.0,
            message_cy,
        ),
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
    let text_left = plate.right() + Style::SP_M;
    let text_right = if stacked {
        card.right() - Style::SP_M
    } else {
        banner_text_right(card, toast.action.is_some())
    }
    .max(text_left);
    let text_bottom = if stacked {
        card.top() + BANNER_H
    } else {
        card.bottom()
    };
    let text_clip = Rect::from_min_max(pos2(text_left, card.top()), pos2(text_right, text_bottom));
    let clipped = painter.with_clip_rect(text_clip);
    clipped.text(
        pos2(text_left, message_cy - Style::SP_XS / 2.0),
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
        pos2(text_left, message_cy + Style::SP_XS / 2.0),
        Align2::LEFT_TOP,
        detail,
        banner_detail_font(),
        Style::TEXT_DIM.gamma_multiply(alpha),
    );

    // Right: dismiss/ack button, optional action button, then countdown + "N more".
    paint_banner_controls(
        ui,
        &painter,
        card,
        banner_control_rect(card),
        toast,
        backlog,
        remaining,
    )
}

/// Paint the dedicated AI operator alert: a centered, red, constrained card
/// over a quiet scrim. It uses the same queue, dwell, hover, action, and
/// acknowledge semantics as every other alert; only the presentation differs.
fn paint_ai_generated_alert(
    ui: &mut Ui,
    toast: &Toast,
    backlog: usize,
    remaining: Option<Duration>,
    t: f32,
) -> BandOutcome {
    let Tier::Alert(severity) = toast.tier else {
        return BandOutcome::default();
    };
    let alpha = t.clamp(0.0, 1.0);
    let screen = ui.ctx().screen_rect();
    let card = ai_alert_rect(screen, t);
    let painter = ui.painter().clone();

    // The full-screen interaction layer prevents an underlying control from
    // receiving the same click while the centered deployment notice is visible.
    let _blocker = ui.interact(
        screen,
        egui::Id::new(AI_ALERT_BLOCKER_ID),
        Sense::click_and_drag(),
    );
    painter.rect_filled(screen, 0.0, Style::SCRIM_REGULAR.gamma_multiply(alpha));

    let radius = Style::RADIUS_XL;
    let mut shadow = Elevation::Overlay.egui_shadow();
    shadow.color = shadow.color.gamma_multiply(alpha);
    painter.add(shadow.as_shape(card, radius));
    painter.rect_filled(card, radius, Style::SUPPORT_ERROR.gamma_multiply(alpha));
    painter.rect_stroke(
        card,
        radius,
        egui::Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.22 * alpha)),
        egui::StrokeKind::Inside,
    );

    let white = Color32::WHITE.gamma_multiply(alpha);
    let muted_white = Color32::WHITE.gamma_multiply(0.78 * alpha);
    let plate = Rect::from_center_size(
        pos2(card.center().x, card.top() + 42.0),
        vec2(BANNER_GLYPH_PLATE, BANNER_GLYPH_PLATE),
    );
    painter.circle_filled(
        plate.center(),
        plate.width() * 0.5,
        Color32::WHITE.gamma_multiply(0.16 * alpha),
    );
    if !paint_carbon(
        &painter,
        plate.shrink(Style::SP_XS),
        severity.glyph_name(),
        white,
    ) {
        painter.circle_filled(plate.center(), Style::SP_XS, white);
    }

    painter.text(
        pos2(card.center().x, plate.bottom() + Style::SP_S),
        Align2::CENTER_TOP,
        AI_GENERATED_ALERT_FLAG,
        Style::typography_font_with_size(TypographyRole::Label, Style::TYPE_FOOTNOTE),
        muted_white,
    );

    let headline_width = 2.0f32.mul_add(-Style::SP_XL, card.width()).max(1.0);
    let mut headline = LayoutJob::single_section(
        toast.headline.clone(),
        TextFormat::simple(
            Style::typography_font_with_size(TypographyRole::Title, Style::TYPE_TITLE3),
            white,
        ),
    );
    headline.wrap.max_width = headline_width;
    headline.wrap.max_rows = 2;
    headline.halign = Align::Center;
    let galley = ui.fonts(|fonts| fonts.layout_job(headline));
    let headline_origin = centered_galley_origin(card, card.top() + 92.0, galley.rect);
    painter.galley(headline_origin, galley, white);

    let mut meta = if toast.source_host.is_empty() {
        String::new()
    } else {
        toast.source_host.clone()
    };
    if let Some(rem) = remaining {
        if !meta.is_empty() {
            meta.push_str("  ·  ");
        }
        meta.push_str(&format!("{:.0}s", rem.as_secs_f32().ceil()));
    } else if matches!(severity, Severity::Critical) {
        if !meta.is_empty() {
            meta.push_str("  ·  ");
        }
        meta.push_str("Acknowledgement required");
    }
    if backlog > 0 {
        if !meta.is_empty() {
            meta.push_str("  ·  ");
        }
        meta.push_str(&format!("{backlog} more"));
    }
    painter.text(
        pos2(card.center().x, card.bottom() - 58.0),
        Align2::CENTER_BOTTOM,
        meta,
        Style::typography_font_with_size(TypographyRole::Label, Style::TYPE_FOOTNOTE),
        muted_white,
    );

    let (label, requires_acknowledgement) = close_control(toast);
    let button_count = if toast.action.is_some() { 2 } else { 1 };
    let gap = Style::SP_S;
    let total_w = AI_ALERT_BUTTON_W.mul_add(button_count as f32, gap * (button_count - 1) as f32);
    let mut x = card.center().x - total_w * 0.5;
    let button_y = card.bottom() - Style::SP_M - AI_ALERT_BUTTON_H;
    let mut out = BandOutcome::default();

    if let Some(action) = &toast.action {
        let rect = Rect::from_min_size(
            pos2(x, button_y),
            vec2(AI_ALERT_BUTTON_W, AI_ALERT_BUTTON_H),
        );
        let response = ui.put(
            rect,
            egui::Button::new(
                Style::typography_text(&action.label, TypographyRole::Label).color(white),
            )
            .fill(Color32::WHITE.gamma_multiply(0.14 * alpha))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::WHITE.gamma_multiply(0.34 * alpha),
            )),
        );
        if response.clicked() {
            out.action = Some(action.verb.clone());
        }
        out.hovered |= response.hovered();
        x = rect.right() + gap;
    }

    let rect = Rect::from_min_size(
        pos2(x, button_y),
        vec2(AI_ALERT_BUTTON_W, AI_ALERT_BUTTON_H),
    );
    let response = ui.put(
        rect,
        egui::Button::new(
            Style::typography_text(label, TypographyRole::Label).color(Style::SUPPORT_ERROR),
        )
        .fill(white),
    );
    if response.clicked() {
        if requires_acknowledgement {
            out.acknowledged = true;
        } else {
            out.dismissed = true;
        }
    }
    out.hovered |= response.hovered();
    let hover = ui.interact(card, egui::Id::new(CHYRON_HOVER_ID), Sense::hover());
    out.hovered |= hover.hovered();
    out
}

/// Paint the card's right-hand controls (dismiss/acknowledge + optional action +
/// countdown/"N more") and report the interaction, including card hover.
fn paint_banner_controls(
    ui: &mut Ui,
    painter: &egui::Painter,
    hover_band: Rect,
    control_band: Rect,
    toast: &Toast,
    backlog: usize,
    remaining: Option<Duration>,
) -> BandOutcome {
    let (label, is_ack) = close_control(toast);
    let cy = control_band.center().y;
    let btn_h = (control_band.height() - Style::SP_M).min(BANNER_H - Style::SP_M);
    let btn_w = BANNER_BUTTON_W;
    let mut rx = control_band.right() - Style::SP_M;
    let mut out = BandOutcome::default();

    let dz = Rect::from_min_max(
        pos2(rx - btn_w, cy - btn_h / 2.0),
        pos2(rx, cy + btn_h / 2.0),
    );
    let dismiss_resp = ui.put(
        dz,
        egui::Button::new(Style::typography_text(label, TypographyRole::Label)),
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
                Style::typography_text(&action.label, TypographyRole::Label).color(Style::BG),
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
    } else if is_ack {
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
            // Fixed-width countdown metadata keeps digits stable-width.
            Style::typography_font_with_size(TypographyRole::Mono, Style::TYPE_FOOTNOTE),
            Style::TEXT_DIM,
        );
    }

    // Hover over the whole band pauses the countdown.
    let hover = ui.interact(hover_band, egui::Id::new(CHYRON_HOVER_ID), Sense::hover());
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
        Style::typography_font_with_size(TypographyRole::Mono, Style::SMALL),
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
        Style::typography_font_with_size(TypographyRole::Mono, Style::SMALL),
        Style::TEXT,
    );
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::egui::FontFamily;

    fn info(host: &str) -> Toast {
        Toast::alert(Severity::Info, host, "CHAT", "a message arrived")
    }

    fn crit(host: &str) -> Toast {
        Toast::alert(Severity::Critical, host, "SECURITY", "intrusion detected")
    }

    // ── model ────────────────────────────────────────────────────────────────

    #[test]
    fn transient_messages_dwell_five_seconds_and_critical_holds() {
        assert_eq!(DWELL_INFO, Duration::from_secs(5));
        assert_eq!(DWELL_WARNING, Duration::from_secs(5));
        assert!(matches!(Severity::Info.dwell(), Dwell::For(DWELL_INFO)));
        assert!(matches!(
            Severity::Warning.dwell(),
            Dwell::For(DWELL_WARNING)
        ));
        assert!(matches!(Severity::Critical.dwell(), Dwell::UntilAck));
        // Both transient tiers use the requested five-second timeout; Critical
        // remains the greatest severity and requires acknowledgement.
        assert_eq!(DWELL_INFO, DWELL_WARNING);
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
    fn delayed_tick_consumes_elapsed_across_timed_queue_but_cannot_cross_ack_hold() {
        let mut host = ToastHost::new();
        host.enqueue(
            Toast::alert(
                Severity::Critical,
                "timed-grade-e",
                "HEALTH · GRADE E",
                "timed critical health alert",
            )
            .with_dwell(Dwell::For(Duration::from_secs(15))),
        );
        host.enqueue(info("second"));
        host.enqueue(crit("held-f-grade"));

        host.tick(Duration::from_secs(60));

        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some("held-f-grade"),
            "elapsed wall time must drain every expired timed alert"
        );
        assert_eq!(host.remaining(), None);
        assert_eq!(host.backlog(), 0);
    }

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
    fn grade_e_timed_critical_cannot_enter_grade_f_acknowledgement_lifecycle() {
        let grade_e = Toast::alert(
            Severity::Critical,
            "workstation-e",
            "HEALTH · GRADE E · 4m 12s · nvme0n1",
            "Storage latency is degrading interactive work",
        )
        .with_dwell(Dwell::For(Duration::from_secs(15)));
        let grade_f = Toast::alert(
            Severity::Critical,
            "workstation-f",
            "HEALTH · GRADE F · 7m 03s · eno1",
            "The node is unreachable",
        )
        .with_dwell(Dwell::UntilAck);

        assert_eq!(close_control(&grade_e), ("Dismiss", false));
        assert_eq!(close_control(&grade_f), ("Acknowledge", true));

        let mut host = ToastHost::new();
        host.enqueue(grade_e);
        host.enqueue(grade_f);
        host.acknowledge();
        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some("workstation-e"),
            "the F-only acknowledgement action must not clear grade E"
        );
        assert_eq!(host.remaining(), Some(Duration::from_secs(15)));

        host.tick(Duration::from_secs(15));
        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some("workstation-f")
        );
        assert_eq!(host.remaining(), None);
        host.acknowledge();
        assert!(host.current().is_none());
    }

    #[test]
    fn lower_generation_health_recovery_cannot_rollback_current_grade_f() {
        let grade_f = Toast::alert(
            Severity::Critical,
            "workstation-7",
            "HEALTH · GRADE F · 7m 03s · eno1",
            "The node is unreachable",
        )
        .with_dwell(Dwell::UntilAck)
        .with_health_authority("network.reachability", 44);
        let stale_recovery = Toast::alert(
            Severity::Info,
            "workstation-7",
            "HEALTH · GRADE A · 2s · eno1",
            "The node recovered",
        )
        .with_dwell(Dwell::For(Duration::from_secs(3)))
        .with_health_authority("network.reachability", 43);
        let conflicting_replay = Toast::alert(
            Severity::Critical,
            "workstation-7",
            "HEALTH · GRADE E · 7m 04s · eno1",
            "A conflicting body reused the current generation",
        )
        .with_dwell(Dwell::For(Duration::from_secs(15)))
        .with_health_authority("network.reachability", 44);

        let mut host = ToastHost::new();
        host.enqueue(grade_f);
        host.enqueue(stale_recovery);
        host.enqueue(conflicting_replay);

        let current = host.current().expect("grade F must remain admitted");
        assert_eq!(current.headline, "The node is unreachable");
        assert_eq!(current.dwell, Dwell::UntilAck);
        assert_eq!(host.backlog(), 0, "stale rollback must not remain queued");
        host.tick(Duration::from_secs(3600));
        host.dismiss();
        assert_eq!(
            host.current().map(|toast| toast.headline.as_str()),
            Some("The node is unreachable"),
            "lower/equal generation health cannot dismiss or downgrade grade F"
        );
    }

    #[test]
    fn cleared_health_authority_retains_bounded_watermark_and_only_moves_forward() {
        fn health(
            host: &str,
            condition: &str,
            generation: u64,
            headline: &str,
            dwell: Dwell,
        ) -> Toast {
            Toast::alert(Severity::Critical, host, "HEALTH", headline)
                .with_dwell(dwell)
                .with_health_authority(condition, generation)
        }

        let mut host = ToastHost::new();

        host.enqueue(health(
            "ack-node",
            "network.reachability",
            40,
            "acknowledged generation",
            Dwell::UntilAck,
        ));
        host.acknowledge();
        host.enqueue(health(
            "ack-node",
            "network.reachability",
            40,
            "conflicting replay after acknowledge",
            Dwell::UntilAck,
        ));
        assert!(host.current().is_none());

        host.enqueue(health(
            "dismiss-node",
            "storage.pressure",
            50,
            "dismissed generation",
            Dwell::For(Duration::from_secs(5)),
        ));
        host.dismiss();
        host.enqueue(health(
            "dismiss-node",
            "storage.pressure",
            49,
            "stale replay after dismiss",
            Dwell::For(Duration::from_secs(5)),
        ));
        assert!(host.current().is_none());

        host.enqueue(health(
            "timeout-node",
            "cpu.temperature",
            60,
            "timed generation",
            Dwell::For(Duration::from_secs(5)),
        ));
        host.tick(Duration::from_secs(5));
        host.enqueue(health(
            "timeout-node",
            "cpu.temperature",
            60,
            "conflicting replay after timeout",
            Dwell::For(Duration::from_secs(5)),
        ));
        assert!(host.current().is_none());

        host.enqueue(health(
            "timeout-node",
            "cpu.temperature",
            61,
            "corrected-forward generation",
            Dwell::For(Duration::from_secs(5)),
        ));
        assert_eq!(
            host.current().map(|toast| toast.headline.as_str()),
            Some("corrected-forward generation")
        );
        assert_eq!(host.health_watermarks.len(), 3);
        assert!(host.health_watermarks.len() <= MAX_HEALTH_AUTHORITY_WATERMARKS);

        host.dismiss();
        for index in 3..MAX_HEALTH_AUTHORITY_WATERMARKS {
            host.enqueue(health(
                "flood-node",
                &format!("hostile.condition.{index}"),
                1,
                "bounded authority",
                Dwell::For(Duration::from_secs(1)),
            ));
            host.dismiss();
        }
        assert_eq!(
            host.health_watermarks.len(),
            MAX_HEALTH_AUTHORITY_WATERMARKS
        );
        host.enqueue(health(
            "overflow-node",
            "unseen.condition",
            1,
            "must fail closed at capacity",
            Dwell::For(Duration::from_secs(1)),
        ));
        assert!(host.current().is_none());
        host.enqueue(health(
            "ack-node",
            "network.reachability",
            39,
            "old authority still cannot roll back",
            Dwell::UntilAck,
        ));
        assert!(host.current().is_none());
    }

    #[test]
    fn rejected_full_backlog_does_not_advance_health_generation_watermark() {
        let mut host = ToastHost::new();
        host.enqueue(crit("visible-critical"));
        for index in 0..MAX_ALERT_BACKLOG {
            host.enqueue(crit(&format!("queued-critical-{index}")));
        }
        assert_eq!(host.backlog(), MAX_ALERT_BACKLOG);

        let health = Toast::alert(
            Severity::Critical,
            "new-health-node",
            "HEALTH",
            "generation must be visible or queued before it is admitted",
        )
        .with_dwell(Dwell::UntilAck)
        .with_health_authority("storage.pressure", 7);
        host.enqueue(health.clone());
        assert!(!host.health_watermarks.iter().any(|watermark| {
            watermark.source_host == "new-health-node"
                && watermark.condition_id == "storage.pressure"
        }));

        host.acknowledge();
        host.enqueue(health);
        assert!(host.health_watermarks.iter().any(|watermark| {
            watermark.source_host == "new-health-node"
                && watermark.condition_id == "storage.pressure"
                && watermark.snapshot_generation == 7
        }));
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
    fn held_health_storm_is_bounded_without_displacing_admitted_critical_alerts() {
        let mut host = ToastHost::new();
        host.enqueue(crit("visible-f-grade"));
        for index in 0..MAX_ALERT_BACKLOG {
            host.enqueue(crit(&format!("queued-f-grade-{index}")));
        }
        host.enqueue(crit("overflow-f-grade"));

        assert_eq!(host.backlog(), MAX_ALERT_BACKLOG);
        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some("visible-f-grade")
        );

        host.acknowledge();
        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some("queued-f-grade-0"),
            "the oldest admitted F-grade alert must remain first"
        );
        for _ in 1..MAX_ALERT_BACKLOG {
            host.acknowledge();
        }
        let last_admitted = format!("queued-f-grade-{}", MAX_ALERT_BACKLOG - 1);
        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some(last_admitted.as_str())
        );
        host.acknowledge();
        assert!(host.current().is_none());
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

    #[test]
    fn ai_generated_operator_notice_preempts_an_ambient_alert() {
        let mut host = ToastHost::new();
        host.enqueue(info("chat-peer"));
        host.enqueue(Toast::alert(
            Severity::Warning,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Update begins in 5 seconds",
        ));
        assert!(
            host.current().is_some_and(Toast::is_ai_generated_alert),
            "the operator notice must be visible immediately"
        );
        assert_eq!(host.backlog(), 1, "the displaced alert remains queued");
    }

    #[test]
    fn ai_generated_operator_notice_preempts_and_preserves_system_critical() {
        let mut host = ToastHost::new();
        host.enqueue(crit("health-monitor"));
        host.enqueue(Toast::alert(
            Severity::Warning,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Update begins in 5 seconds",
        ));

        assert!(
            host.current().is_some_and(Toast::is_ai_generated_alert),
            "the safety notice must not wait behind an UntilAck alert"
        );
        assert_eq!(host.backlog(), 1, "the displaced Critical remains queued");

        host.tick(DWELL_WARNING);
        assert!(host.has_critical(), "the displaced Critical must resume");
        assert_eq!(
            host.current().map(|toast| toast.source_host.as_str()),
            Some("health-monitor")
        );
        assert_eq!(host.remaining(), None, "the Critical must retain UntilAck");
    }

    #[test]
    fn ai_generated_warning_preempts_an_ai_generated_critical() {
        let mut host = ToastHost::new();
        host.enqueue(Toast::alert(
            Severity::Critical,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Previous operation requires acknowledgement",
        ));
        host.enqueue(Toast::alert(
            Severity::Warning,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Update begins in 5 seconds",
        ));

        let current = host.current().expect("the new warning must be current");
        assert_eq!(current.tier, Tier::Alert(Severity::Warning));
        assert_eq!(current.headline, "Update begins in 5 seconds");
        assert_eq!(host.backlog(), 1, "the Critical remains in the backlog");
    }

    #[test]
    fn displaced_ai_generated_critical_returns_and_still_requires_acknowledgement() {
        let mut host = ToastHost::new();
        host.enqueue(Toast::alert(
            Severity::Critical,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Previous operation requires acknowledgement",
        ));
        host.enqueue(Toast::alert(
            Severity::Warning,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Update begins in 5 seconds",
        ));

        host.tick(DWELL_WARNING);
        assert!(
            host.has_critical(),
            "the displaced Critical must return next"
        );
        assert_eq!(
            host.current().map(|toast| toast.headline.as_str()),
            Some("Previous operation requires acknowledgement")
        );
        assert_eq!(host.remaining(), None, "the Critical must retain UntilAck");

        host.tick(Duration::from_secs(3600));
        host.dismiss();
        assert!(
            host.has_critical(),
            "tick and dismiss cannot clear the Critical"
        );
        host.acknowledge();
        assert!(
            host.current().is_none(),
            "acknowledgement clears the Critical"
        );
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

    #[test]
    fn ai_generated_alert_flag_selects_the_dedicated_presentation() {
        let exact = Toast::alert(
            Severity::Warning,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Update begins in 5 seconds",
        );
        let mixed_case = Toast::alert(
            Severity::Warning,
            "controller",
            "ai-generated-alert",
            "Update begins in 5 seconds",
        );
        assert!(exact.is_ai_generated_alert());
        assert!(mixed_case.is_ai_generated_alert());
        assert!(!info("controller").is_ai_generated_alert());
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
    fn banner_is_wider_and_text_never_enters_the_control_lane() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 720.0));
        let card = banner_rect(screen, 1.0);
        assert_eq!(card.width(), BANNER_MAX_W);
        assert!(
            card.width() > 520.0,
            "desktop messages must have more room than the retired 520px banner"
        );

        for has_action in [false, true] {
            let text_right = banner_text_right(card, has_action);
            let button_count = if has_action { 2.0 } else { 1.0 };
            let button_gaps = if has_action { Style::SP_S } else { 0.0 };
            let control_lane_left = card.right()
                - Style::SP_M
                - BANNER_BUTTON_W * button_count
                - button_gaps
                - Style::SP_S
                - BANNER_META_W;
            assert!(
                text_right <= control_lane_left - Style::SP_M,
                "message text must stop before metadata and controls"
            );
        }
    }

    #[test]
    fn narrow_banner_stacks_message_above_controls() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0));
        let card = banner_rect(screen, 1.0);
        let controls = banner_control_rect(card);
        assert!(banner_is_stacked(card));
        assert_eq!(card.height(), BANNER_STACKED_H);
        assert_eq!(controls.top(), card.top() + BANNER_H);
        assert_eq!(controls.bottom(), card.bottom());
        assert!(controls.top() >= card.top() + BANNER_H);
    }

    #[test]
    fn ai_generated_alert_is_centered_and_constrained_on_wide_and_narrow_screens() {
        for screen in [
            Rect::from_min_size(egui::Pos2::ZERO, vec2(1920.0, 1080.0)),
            Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0)),
        ] {
            let card = ai_alert_rect(screen, 1.0);
            assert!((card.center().x - screen.center().x).abs() < f32::EPSILON);
            assert!((card.center().y - screen.center().y).abs() < f32::EPSILON);
            assert!(card.width() <= AI_ALERT_MAX_W);
            assert!(card.width() <= screen.width() - 2.0 * AI_ALERT_MARGIN + f32::EPSILON);
            assert!(card.height() <= screen.height() - 2.0 * AI_ALERT_MARGIN + f32::EPSILON);
        }
        let desktop = Rect::from_min_size(egui::Pos2::ZERO, vec2(1920.0, 1080.0));
        assert_eq!(ai_alert_rect(desktop, 1.0).width(), AI_ALERT_MAX_W);
        assert!(
            AI_ALERT_MAX_W > 460.0,
            "operator messages must have more room than the retired 460px card"
        );
    }

    #[test]
    fn ai_generated_alert_centers_the_center_aligned_headline_bounds() {
        let card = Rect::from_center_size(pos2(960.0, 540.0), vec2(460.0, 224.0));
        // `LayoutJob::halign = Align::Center` lays a galley out around x=0,
        // rather than from x=0. The live seat bug double-subtracted half its
        // width and shifted the headline outside the left edge of the card.
        let galley_rect = Rect::from_min_max(pos2(-198.0, 0.0), pos2(198.0, 24.0));
        let origin = centered_galley_origin(card, card.top() + 92.0, galley_rect);
        let painted = galley_rect.translate(origin.to_vec2());

        assert!((painted.center().x - card.center().x).abs() < f32::EPSILON);
        assert!(painted.left() >= card.left());
        assert!(painted.right() <= card.right());
    }

    #[test]
    fn ai_generated_alert_uses_the_only_layer_above_the_lock_curtain() {
        let operator_notice = Toast::alert(
            Severity::Warning,
            "controller",
            AI_GENERATED_ALERT_FLAG,
            "Update begins in 5 seconds",
        );
        assert_eq!(chyron_order(&operator_notice), egui::Order::Tooltip);
        assert_eq!(chyron_order(&info("ordinary")), egui::Order::Foreground);
    }

    #[test]
    fn banner_type_reads_body_over_footnote() {
        // Q14: the HIG type ladder — Body title over Label detail.
        assert_eq!(
            banner_title_font(),
            Style::typography_font(TypographyRole::Body)
        );
        assert_eq!(
            banner_detail_font(),
            Style::typography_font(TypographyRole::Label)
        );
    }

    #[test]
    fn technical_toast_text_remains_monospace() {
        assert_eq!(
            Style::typography_font_with_size(TypographyRole::Mono, Style::TYPE_FOOTNOTE).family,
            FontFamily::Monospace
        );
        assert_eq!(
            Style::typography_font_with_size(TypographyRole::Mono, Style::SMALL).family,
            FontFamily::Monospace
        );
    }

    /// Optional render-proof fixture for the two popup presentations. Normal
    /// test runs stay filesystem-free; GUI proof jobs opt in with an output dir.
    #[test]
    fn render_wide_popup_message_proofs_when_requested() {
        let Some(output_dir) = std::env::var_os("MDE_TOAST_PROOF_DIR") else {
            return;
        };
        let output_dir = std::path::PathBuf::from(output_dir);
        std::fs::create_dir_all(&output_dir).expect("create toast proof directory");
        let size = vec2(1280.0, 720.0);

        let banner = Toast::alert(
            Severity::Warning,
            "Basement-Test-Workstation",
            "SYSTEM",
            "Firmware metadata refresh completed successfully after provider recovery",
        )
        .with_action("Open", "shell/goto/system");
        let png = crate::capture::capture_ui_png(size, 1.0, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Style::BG))
                .show(ctx, |ui| {
                    let _ = paint_banner(ui, &banner, 2, Some(Duration::from_secs(5)), 1.0);
                });
        })
        .expect("render standard popup proof");
        std::fs::write(output_dir.join("popup-message-wide-banner.png"), png)
            .expect("write standard popup proof");

        let narrow_size = vec2(400.0, 300.0);
        let png = crate::capture::capture_ui_png(narrow_size, 1.0, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Style::BG))
                .show(ctx, |ui| {
                    let _ = paint_banner(ui, &banner, 2, Some(Duration::from_secs(5)), 1.0);
                });
        })
        .expect("render narrow popup proof");
        std::fs::write(output_dir.join("popup-message-narrow-banner.png"), png)
            .expect("write narrow popup proof");

        let operator = Toast::alert(
            Severity::Warning,
            "deployment-controller",
            AI_GENERATED_ALERT_FLAG,
            "The corrected preview will restart the desktop shell on all five seats",
        )
        .with_action("Review", "shell/goto/notifications");
        let png = crate::capture::capture_ui_png(size, 1.0, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Style::BG))
                .show(ctx, |ui| {
                    let _ = paint_ai_generated_alert(
                        ui,
                        &operator,
                        1,
                        Some(Duration::from_secs(5)),
                        1.0,
                    );
                });
        })
        .expect("render centered popup proof");
        std::fs::write(output_dir.join("popup-message-wide-operator.png"), png)
            .expect("write centered popup proof");
    }
}
