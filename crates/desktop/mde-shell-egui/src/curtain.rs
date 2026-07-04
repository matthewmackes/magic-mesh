//! CURTAIN-1 — the shell's **lock curtain**: the full-screen lock layer, its
//! slide + settle-bounce motion, and the giant clock face
//! (`docs/design/lock-curtain.md`, locks 5/6/8/10).
//!
//! The DM-less DRM shell boots straight to the desktop, so the curtain is the
//! seat's ONE lock layer: a whole-screen top-most sheet that drops from the top
//! edge with a slight settle overshoot (lock 5), rests as a giant digital
//! clock face (lock 6), reveals a password stage on any key/click (lock 8),
//! and — while engaged — consumes ALL input (lock 10): the pointer through the
//! covering `Order::Foreground` layer (re-raised with `move_to_top` every
//! frame, so no chyron/OSK float outranks it), the keyboard through a
//! per-frame focus steal here plus the hotkey / edge-swipe / central-view
//! gates in `main.rs`.
//!
//! The state machine is the design's `Unlocked → Dropping → Locked(face) →
//! Revealing(password) → Verifying → (Lifting | Backoff)` and is pure —
//! [`Curtain::tick`] advances the per-state timers, typed [`Signals`] folds
//! carry the input — so every transition (including the 5-deny backoff wall,
//! its live countdown, and the idle dim) is unit-tested without a GPU.
//!
//! **CURTAIN-1 has no authenticator.** Verification runs through the
//! [`Verifier`] seam CURTAIN-2 fills with the real PAM conversation (begun
//! off-thread, polled per tick); the default [`NotWired`] verifier honestly
//! denies every attempt — it can never unlock (§7 forbids a pretend-success),
//! so a locked CURTAIN-1 seat stays locked until the shell service restarts,
//! and the deny line says exactly that. Tests unlock through the seam with a
//! scripted verdict. The status row + unified now-playing strip are the
//! CURTAIN-4 seam ([`Curtain::face_extras`]), deliberately empty here.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, Splash, …); the shell in main.rs consumes them"
)]

use std::time::{Duration, Instant};

use mde_egui::egui::{self, Align2, Color32, FontId, RichText};
use mde_egui::{Motion, Style};

// ───────────────────────────── the verify seam ─────────────────────────────

/// One verification attempt's verdict, delivered through the [`Verifier`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The password verified — the curtain lifts.
    #[allow(
        dead_code,
        reason = "Granted is produced by the CURTAIN-2 PAM verifier (and the tests' \
                  scripted seam); CURTAIN-1's only in-tree verifier honestly denies"
    )]
    Granted,
    /// The attempt was denied, with the honest reason line the stage shows.
    Denied(String),
}

/// The **CURTAIN-2 seam**: the curtain never checks a password itself — it
/// hands each attempt to this verifier and polls for the verdict. CURTAIN-2
/// fills it with the real PAM conversation for the seat user (begun on a
/// worker thread so the render loop never blocks on `pam_authenticate`, the
/// pairing-dialog channel-bridge pattern); tests fill it with scripted
/// verdicts. The default is [`NotWired`], which denies everything.
pub(crate) trait Verifier {
    /// Start verifying `password` for the seat user. Must not block — a real
    /// implementation moves the work off-thread and answers through [`poll`].
    ///
    /// [`poll`]: Verifier::poll
    fn begin(&mut self, password: &str);

    /// The in-flight attempt's verdict, once it lands (`None` while running).
    fn poll(&mut self) -> Option<Verdict>;
}

/// The honest placeholder verifier: CURTAIN-1 ships the curtain without an
/// authenticator, so every attempt is **denied** with the reason. It is
/// deliberately incapable of producing [`Verdict::Granted`] — a lock that
/// pretends to verify would be a §7 mockup in the security path.
#[derive(Debug, Default)]
struct NotWired {
    /// An attempt was begun and its (always-deny) verdict is ready to poll.
    pending: bool,
}

/// The [`NotWired`] verifier's deny line — honest about why nothing unlocks.
const NOT_WIRED_DENY: &str = "Unlock is not wired to the system password yet (CURTAIN-2): \
     this curtain cannot verify passwords and never pretends to. \
     Restart the shell service to regain the seat.";

impl Verifier for NotWired {
    fn begin(&mut self, _password: &str) {
        self.pending = true;
    }

    fn poll(&mut self) -> Option<Verdict> {
        if !self.pending {
            return None;
        }
        self.pending = false;
        Some(Verdict::Denied(NOT_WIRED_DENY.to_owned()))
    }
}

// ───────────────────────────── the state machine ─────────────────────────────

/// The curtain's phase — the design's state machine, with each phase carrying
/// its own timer so [`Curtain::tick`] is the whole clock.
#[derive(Debug)]
enum Phase {
    /// No curtain: the seat is the operator's; the shell routes input normally.
    Unlocked,
    /// The sheet is sliding down from the top edge (lock 5); `p` is the linear
    /// motion progress in `0.0..1.0` (the overshoot lives in [`drop_offset`]).
    Dropping {
        /// Linear drop progress, `0.0..1.0`.
        p: f32,
    },
    /// The settled clock face; `idle_secs` accumulates toward the idle dim.
    Locked {
        /// Seconds since the last user activity (any input resets it).
        idle_secs: f32,
    },
    /// The password stage is up (two-stage reveal, lock 8).
    Revealing,
    /// An attempt is with the [`Verifier`]; its verdict is polled per tick.
    Verifying,
    /// The sheet is lifting up and out after a grant; `p` as in `Dropping`.
    Lifting {
        /// Linear lift progress, `0.0..1.0`.
        p: f32,
    },
    /// The backoff wall after [`MAX_FAILS`] denies (lock 10): the field is
    /// disabled while `remaining` counts down, live, to a fresh window.
    Backoff {
        /// Cooldown seconds left.
        remaining: f32,
    },
}

/// The drop's duration in seconds — the ~300 ms slide on the shared Motion
/// table (lock 5).
const DROP_SECS: f32 = Motion::SLOW;
/// The lift's duration in seconds (the unlock mirrors the drop's pace).
const LIFT_SECS: f32 = Motion::SLOW;
/// Idle seconds on the face before it dims to near-black (design: ~30 s).
const IDLE_DIM_SECS: f32 = 30.0;
/// Failed attempts that arm the backoff wall (design lock 10).
const MAX_FAILS: u32 = 5;
/// The backoff cooldown in seconds (design lock 10).
const BACKOFF_SECS: f32 = 30.0;
/// The largest single step the motion timers accept — a hitchy frame (a slow
/// poll, a debugger pause) advances the slide smoothly instead of teleporting
/// it. The idle timer and the backoff countdown deliberately take raw elapsed
/// time (they must track the wall clock across the 1 Hz locked cadence).
const MAX_MOTION_DT: f32 = 0.25;

// ─────────────────────────── face metrics (tokens) ───────────────────────────

/// The giant clock's point size — a display-scale multiple of the shared type
/// scale (lock 6; the §4 tokens carry no display tier, so the face scales the
/// heading token rather than minting a raw size).
const CLOCK_PT: f32 = Style::HEADING * 6.0;
/// The date line's point size beneath the clock — the same token, display-scaled.
const DATE_PT: f32 = Style::HEADING * 1.5;
/// The password field's width, on the spacing grid.
const FIELD_W: f32 = Style::SP_XL * 8.0;
/// The password stage's row height, on the spacing grid.
const ROW_H: f32 = Style::SP_XL * 3.0;
/// The clock's centre line as a fraction of the sheet height (centred-high).
const CLOCK_Y_FRAC: f32 = 0.28;
/// The CURTAIN-4 extras region's centre line (between the date and the stage).
const EXTRAS_Y_FRAC: f32 = 0.55;
/// The password stage's centre line (low-centre, lock 8).
const FIELD_Y_FRAC: f32 = 0.75;
/// The extras region's width as a fraction of the sheet width.
const EXTRAS_W_FRAC: f32 = 0.5;
/// The extras region's height as a fraction of the sheet height.
const EXTRAS_H_FRAC: f32 = 0.12;
/// How far the idle dim drops the face type toward black — the clock stays
/// faint, never gone (design lock 10).
const FAINT_DROP: f32 = 0.8;
/// How far the idle dim drops the sheet fill toward black (near-black; the
/// fill stays fully opaque — a dimmed curtain must not become a window).
const BG_DROP: f32 = 0.9;
/// The sheet's paint bleed above its top edge, as a fraction of the sheet
/// height: the drop's settle overshoot peaks at ≈ +10 % of the height, and the
/// bleed keeps the desktop covered through it.
const OVERSHOOT_BLEED: f32 = 0.15;

/// The curtain layer's area id.
const CURTAIN_AREA: &str = "shell-curtain";
/// The password field's stable widget id — the ONE id the focus steal spares.
const PASSWORD_FIELD: &str = "curtain-password";
/// The two-stage reveal's animation key (the field's slide/fade).
const REVEAL_KEY: &str = "curtain-reveal";
/// The idle dim's animation key (the fade to near-black).
const DIM_KEY: &str = "curtain-dim";

// ─────────────────────────────── input folding ───────────────────────────────

/// The curtain's typed reading of one frame's raw input.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Signals {
    /// Any genuine user activity — wakes the idle dim.
    wake: bool,
    /// A key / text / click — reveals the password stage from the face (lock 8).
    reveal: bool,
    /// The Escape key — drops the password stage back to the face (lock 8).
    escape: bool,
}

/// Fold one frame's egui events into the curtain's [`Signals`]. A pointer move
/// (or wheel/zoom/touch drift) only wakes the dim; a key press, text, paste, or
/// button press also reveals the stage; releases and non-user events (a focus
/// flip, `PointerGone`) are nothing — never a phantom reveal.
fn fold_events(events: &[egui::Event]) -> Signals {
    let mut s = Signals::default();
    for e in events {
        match e {
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            } => {
                s.escape = true;
                s.reveal = true; // any key is still "a key" on the face
                s.wake = true;
            }
            egui::Event::Key { pressed: true, .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::PointerButton { pressed: true, .. } => {
                s.reveal = true;
                s.wake = true;
            }
            egui::Event::PointerMoved(_)
            | egui::Event::MouseMoved(_)
            | egui::Event::MouseWheel { .. }
            | egui::Event::Zoom(_)
            | egui::Event::Touch { .. } => s.wake = true,
            _ => {}
        }
    }
    s
}

// ────────────────────────────── motion (lock 5) ──────────────────────────────

/// The drop's y-offset as a fraction of the sheet height for linear progress
/// `p`: `-1.0` = resting fully above the screen, `0.0` = covering exactly. The
/// ease-out-back curve carries the settle bounce — the sheet overshoots
/// slightly **past** the bottom (offset > 0, peak ≈ +0.10) and settles back to
/// exactly `0.0` at `p = 1.0`.
fn drop_offset(p: f32) -> f32 {
    /// Ease-out-back's overshoot constant (the canonical ~10 % settle).
    const C1: f32 = 1.701_58;
    /// Ease-out-back's cubic constant.
    const C3: f32 = C1 + 1.0;
    let q = p.clamp(0.0, 1.0) - 1.0;
    // easeOutBack(p) − 1  =  C3·q³ + C1·q²   (q = p − 1)
    C3.mul_add(q, C1) * q * q
}

/// The lift's y-offset for linear progress `p`: `0.0` = covering, `-1.0` =
/// fully up and out. Cubic ease-in — the sheet peels off slowly, then
/// accelerates up and away (lock 5's "lifts up and out").
fn lift_offset(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    -(p * p * p)
}

// ─────────────────────────────── the clock face ───────────────────────────────

/// The face's two lines (lock 6): wall-clock `HH:MM` over the civil
/// `YYYY-MM-DD`, UTC — the same no-time-crate calendar fold the Chat timeline
/// and the tray's stacked clock run ([`crate::chat::civil_from_days`] is the
/// crate's ONE calendar, §6; the tray's own `clock_lines` is module-private,
/// so the fold is restated here rather than reaching into `tray.rs`).
fn face_lines(unix_secs: i64) -> (String, String) {
    let tod = unix_secs.rem_euclid(86_400);
    let (year, month, day) = crate::chat::civil_from_days(unix_secs.div_euclid(86_400));
    (
        format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60),
        format!("{year:04}-{month:02}-{day:02}"),
    )
}

/// Seconds since the Unix epoch (0 on a pre-epoch clock — same guard as the
/// tray's clock cell).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Fade a face colour toward faint for the idle dim (`dim` in `0.0..=1.0`).
/// Premultiplied fade — the type dims over the near-black sheet, never gone.
fn faint(c: Color32, dim: f32) -> Color32 {
    c.gamma_multiply(dim.mul_add(-FAINT_DROP, 1.0))
}

/// Fold the sheet fill toward near-black for the idle dim, **keeping it fully
/// opaque** — a dimmed curtain must never become a window onto the desktop.
fn near_black(c: Color32, dim: f32) -> Color32 {
    let k = dim.mul_add(-BG_DROP, 1.0);
    Color32::from_rgb(scale8(c.r(), k), scale8(c.g(), k), scale8(c.b(), k))
}

/// Scale one 8-bit channel by `k` (`0.0..=1.0`).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "k is clamped non-negative and ≤ 1, so v·k stays in 0..=255"
)]
fn scale8(v: u8, k: f32) -> u8 {
    (f32::from(v) * k.clamp(0.0, 1.0)).round() as u8
}

// ─────────────────────────────── the curtain ───────────────────────────────

/// The lock curtain: the pure state machine plus its egui face. `main.rs`
/// mounts it last in the shell render and gates hotkeys / edge swipes / the
/// central view on [`Curtain::engaged`] / [`Curtain::covers_fully`]; everything
/// else lives here.
pub(crate) struct Curtain {
    /// The current phase (the state machine's node + its timer).
    phase: Phase,
    /// Consecutive denied attempts in the current window (lock 10).
    fails: u32,
    /// The honest error line the stage shows after a deny.
    error: Option<String>,
    /// The password buffer behind the stage's field; cleared on submit,
    /// escape, and dismissal — it never outlives the attempt.
    password: String,
    /// The verify seam (CURTAIN-2 fills it with PAM; [`NotWired`] by default).
    verifier: Box<dyn Verifier>,
    /// The previous `show` frame's instant — the real-elapsed clock behind
    /// [`Curtain::tick`] (frames run at 1 Hz on the settled face, so egui's
    /// smoothed `stable_dt` would under-count the idle timer).
    last_frame: Option<Instant>,
}

impl Default for Curtain {
    fn default() -> Self {
        Self::with_verifier(Box::new(NotWired::default()))
    }
}

impl Curtain {
    /// Build a curtain over an explicit [`Verifier`] — the constructor
    /// CURTAIN-2 calls with the real PAM seam (and tests call with a scripted
    /// one). [`Default`] routes here with the honest [`NotWired`] deny-all.
    pub(crate) fn with_verifier(verifier: Box<dyn Verifier>) -> Self {
        Self {
            phase: Phase::Unlocked,
            fails: 0,
            error: None,
            password: String::new(),
            verifier,
            last_frame: None,
        }
    }

    /// Build the curtain over the **real seat-user authenticator** — the
    /// CURTAIN-2 PAM path ([`crate::pam_auth::PamVerifier`]), which verifies each
    /// unlock against the seat user's system password off the render thread. The
    /// constructor the shell mounts in `main.rs`; [`Default`] keeps the honest
    /// deny-all [`NotWired`] seam for tests (and any not-yet-wired build).
    pub(crate) fn pam() -> Self {
        Self::with_verifier(Box::new(crate::pam_auth::PamVerifier::new()))
    }

    /// Drop the curtain (Super+L, and later the CURTAIN-3 triggers). Starts
    /// the slide from the top edge; a no-op while already engaged.
    pub(crate) const fn lock(&mut self) {
        if matches!(self.phase, Phase::Unlocked) {
            self.phase = Phase::Dropping { p: 0.0 };
        }
    }

    /// Whether the curtain is anywhere but `Unlocked` — the input-exclusivity
    /// gate (lock 10): while `true`, `main.rs` swallows hotkey actions and
    /// edge swipes, and this module claims the pointer + keyboard.
    pub(crate) const fn engaged(&self) -> bool {
        !matches!(self.phase, Phase::Unlocked)
    }

    /// Whether the settled sheet fully covers the seat (every phase between
    /// the drop and the lift). While `true`, the shell's central view renders
    /// nothing — surfaces beneath an opaque curtain must not run their raw
    /// input reads (the VDI guest forward reads `ui.input` directly), and
    /// there is nothing of them to see. The drop/lift tweens keep the view
    /// beneath the sliding sheet.
    pub(crate) const fn covers_fully(&self) -> bool {
        matches!(
            self.phase,
            Phase::Locked { .. } | Phase::Revealing | Phase::Verifying | Phase::Backoff { .. }
        )
    }

    /// Advance the per-state timers by `dt` seconds — the machine's whole
    /// clock, pure and unit-testable. Motion phases clamp a single step to
    /// [`MAX_MOTION_DT`] so a hitchy frame can't teleport the slide; the idle
    /// timer and backoff countdown take raw elapsed time.
    fn tick(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        match &mut self.phase {
            Phase::Unlocked | Phase::Revealing => {}
            Phase::Dropping { p } => {
                *p += dt.min(MAX_MOTION_DT) / DROP_SECS;
                if *p >= 1.0 {
                    self.phase = Phase::Locked { idle_secs: 0.0 };
                }
            }
            Phase::Locked { idle_secs } => *idle_secs += dt,
            Phase::Verifying => {
                if let Some(verdict) = self.verifier.poll() {
                    self.settle(verdict);
                }
            }
            Phase::Lifting { p } => {
                *p += dt.min(MAX_MOTION_DT) / LIFT_SECS;
                if *p >= 1.0 {
                    self.phase = Phase::Unlocked;
                }
            }
            Phase::Backoff { remaining } => {
                *remaining -= dt;
                if *remaining <= 0.0 {
                    // The wall falls to a fresh attempt window.
                    self.fails = 0;
                    self.error = None;
                    self.phase = Phase::Revealing;
                }
            }
        }
    }

    /// Land a verify verdict: a grant starts the lift; a deny returns to the
    /// stage with the honest reason, and the [`MAX_FAILS`]th deny arms the
    /// backoff wall (lock 10).
    fn settle(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Granted => {
                self.fails = 0;
                self.error = None;
                self.phase = Phase::Lifting { p: 0.0 };
            }
            Verdict::Denied(why) => {
                self.fails += 1;
                if self.fails >= MAX_FAILS {
                    self.error = None;
                    self.phase = Phase::Backoff {
                        remaining: BACKOFF_SECS,
                    };
                } else {
                    self.error = Some(why);
                    self.phase = Phase::Revealing;
                }
            }
        }
    }

    /// Apply one frame's typed input [`Signals`] to the machine: any activity
    /// wakes the idle dim; on the face a key/click reveals the stage; on the
    /// stage Escape drops back to the face (lock 8).
    fn fold_input(&mut self, signals: Signals) {
        if signals.wake {
            self.wake();
        }
        match self.phase {
            Phase::Locked { .. } if signals.reveal => self.reveal(),
            Phase::Revealing if signals.escape => self.escape(),
            _ => {}
        }
    }

    /// Reset the face's idle timer (any user activity).
    const fn wake(&mut self) {
        if let Phase::Locked { idle_secs } = &mut self.phase {
            *idle_secs = 0.0;
        }
    }

    /// Slide the password stage in from the face (lock 8's first key/click).
    fn reveal(&mut self) {
        if matches!(self.phase, Phase::Locked { .. }) {
            self.password.clear();
            self.error = None;
            self.phase = Phase::Revealing;
        }
    }

    /// Drop the stage back to the face (Esc); the buffer never survives it.
    fn escape(&mut self) {
        if matches!(self.phase, Phase::Revealing) {
            self.password.clear();
            self.error = None;
            self.phase = Phase::Locked { idle_secs: 0.0 };
        }
    }

    /// Submit the typed password to the [`Verifier`] seam (Enter on the
    /// stage). The buffer is handed over and cleared immediately; an empty
    /// buffer never submits.
    fn submit(&mut self) {
        if !matches!(self.phase, Phase::Revealing) || self.password.is_empty() {
            return;
        }
        let password = std::mem::take(&mut self.password);
        self.verifier.begin(&password);
        self.error = None;
        self.phase = Phase::Verifying;
    }

    /// Whether the stage's field accepts typing — only while `Revealing`
    /// (disabled through a verify and for the whole backoff wall, lock 10).
    const fn stage_accepts_input(&self) -> bool {
        matches!(self.phase, Phase::Revealing)
    }

    /// The backoff wall's remaining cooldown, while it stands.
    const fn backoff_remaining(&self) -> Option<f32> {
        match self.phase {
            Phase::Backoff { remaining } => Some(remaining),
            _ => None,
        }
    }

    /// Whether the face has idled past [`IDLE_DIM_SECS`] (the dim's target;
    /// the visual fade eases through the Motion table in `show`).
    fn is_dimmed(&self) -> bool {
        matches!(self.phase, Phase::Locked { idle_secs } if idle_secs >= IDLE_DIM_SECS)
    }

    /// The sheet's current y-offset as a fraction of the screen height:
    /// `-1.0` = resting above, `0.0` = covering (with the drop's settle
    /// overshoot briefly > 0).
    fn offset_fraction(&self) -> f32 {
        match self.phase {
            Phase::Unlocked => -1.0,
            Phase::Dropping { p } => drop_offset(p),
            Phase::Lifting { p } => lift_offset(p),
            Phase::Locked { .. } | Phase::Revealing | Phase::Verifying | Phase::Backoff { .. } => {
                0.0
            }
        }
    }

    // ──────────────────────────── the egui face ────────────────────────────

    /// Drive one frame: fold this frame's input, advance the clock, and render
    /// the covering layer. Called LAST in the shell render so `move_to_top`
    /// leaves the curtain above every other float; an early no-op while
    /// `Unlocked`.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        if !self.engaged() {
            self.last_frame = None;
            return;
        }

        // 1 — this frame's raw input, folded to the curtain's typed signals.
        let signals = ctx.input(|i| fold_events(&i.events));
        self.fold_input(signals);

        // 2 — advance the per-state timers on real elapsed time (the settled
        // face repaints at 1 Hz, so egui's smoothed dt would under-count).
        let now = Instant::now();
        let dt = self.last_frame.map_or(0.0, |last| {
            now.saturating_duration_since(last).as_secs_f32()
        });
        self.last_frame = Some(now);
        self.tick(dt);
        if !self.engaged() {
            // The lift completed this frame — the seat is the operator's again.
            self.last_frame = None;
            return;
        }

        // 3 — keyboard exclusivity (lock 10): nothing beneath the curtain
        // keeps focus; the ONE id spared is the curtain's own password field.
        let field = egui::Id::new(PASSWORD_FIELD);
        ctx.memory_mut(|m| {
            if let Some(focused) = m.focused() {
                if focused != field {
                    m.surrender_focus(focused);
                }
            }
        });

        // 4 — the covering layer: one whole-screen Foreground area whose
        // widget claims the full screen rect, so egui's hit-test routes every
        // pointer event here and none to the dock / surfaces beneath (lock
        // 10). The painted sheet rides the motion offset inside the claim.
        let screen = ctx.screen_rect();
        let dim = Motion::animate(ctx, DIM_KEY, self.is_dimmed(), Motion::SLOW);
        let offset = self.offset_fraction() * screen.height();
        let layer = egui::Area::new(egui::Id::new(CURTAIN_AREA))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, _claim) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                let sheet = rect.translate(egui::vec2(0.0, offset));
                self.paint_sheet(ui, sheet, dim);
            });
        ctx.move_to_top(layer.response.layer_id);

        // 5 — repaint: full-rate through the tweens and the verify spinner, a
        // ~10 Hz cadence for the live countdown, 1 Hz on the settled face (the
        // minute flip + the idle-dim threshold).
        match self.phase {
            Phase::Dropping { .. } | Phase::Lifting { .. } | Phase::Verifying => {
                ctx.request_repaint();
            }
            Phase::Backoff { .. } => ctx.request_repaint_after(Duration::from_millis(100)),
            Phase::Locked { .. } | Phase::Revealing => {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            Phase::Unlocked => {}
        }
    }

    /// Paint the sheet into its (offset) rect: the opaque near-black fill with
    /// the overshoot bleed, the giant clock face, the CURTAIN-4 extras seam,
    /// and the two-stage password reveal.
    fn paint_sheet(&mut self, ui: &mut egui::Ui, sheet: egui::Rect, dim: f32) {
        let painter = ui.painter().clone();

        // The sheet fill, bled above the top edge so the drop's settle
        // overshoot (offset briefly > 0) never exposes the desktop above it.
        let fill = egui::Rect::from_min_max(
            egui::pos2(
                sheet.left(),
                OVERSHOOT_BLEED.mul_add(-sheet.height(), sheet.top()),
            ),
            sheet.max,
        );
        painter.rect_filled(fill, 0.0, near_black(Style::BG, dim));

        // The giant clock face (lock 6): thin-reading display-scale HH:MM
        // centred-high, the civil date beneath — UTC, the crate's one calendar.
        let (time, date) = face_lines(unix_now());
        let cx = sheet.center().x;
        let clock_rect = painter.text(
            egui::pos2(cx, sheet.height().mul_add(CLOCK_Y_FRAC, sheet.top())),
            Align2::CENTER_CENTER,
            time,
            FontId::proportional(CLOCK_PT),
            faint(Style::TEXT, dim),
        );
        painter.text(
            egui::pos2(cx, clock_rect.bottom() + Style::SP_M),
            Align2::CENTER_TOP,
            date,
            FontId::proportional(DATE_PT),
            faint(Style::TEXT_DIM, dim),
        );

        // The CURTAIN-4 seam: the glanceable status row + the unified
        // now-playing strip mount in this region (between the date line and
        // the password stage). Deliberately empty in CURTAIN-1.
        let extras = egui::Rect::from_center_size(
            egui::pos2(cx, sheet.height().mul_add(EXTRAS_Y_FRAC, sheet.top())),
            egui::vec2(
                sheet.width() * EXTRAS_W_FRAC,
                sheet.height() * EXTRAS_H_FRAC,
            ),
        );
        let mut extras_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(extras)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        self.face_extras(&mut extras_ui);

        // The two-stage reveal (lock 8): the password stage slides in
        // low-centre. It mounts the instant the stage phase arrives (so the
        // field owns the keyboard from the first frame) and eases its
        // rise/fade through the Motion table.
        let staged = matches!(
            self.phase,
            Phase::Revealing | Phase::Verifying | Phase::Backoff { .. }
        );
        let t = Motion::animate(ui.ctx(), REVEAL_KEY, staged, Motion::BASE);
        if staged || t > 0.01 {
            let rise = (1.0 - t) * Style::SP_XL;
            let row = egui::Rect::from_center_size(
                egui::pos2(cx, sheet.height().mul_add(FIELD_Y_FRAC, sheet.top()) + rise),
                egui::vec2(Style::SP_XL.mul_add(2.0, FIELD_W), ROW_H),
            );
            let mut stage_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(row)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            stage_ui.set_opacity(t.max(0.01));
            self.password_stage(&mut stage_ui);
        }
    }

    /// The **CURTAIN-4 seam** — the face's glanceable extras region: the
    /// status row (battery / mesh health, design lock 4) and the unified
    /// now-playing transport strip (lock 3/7) mount here. CURTAIN-1
    /// deliberately leaves it empty; the region + call site are the extension
    /// point so CURTAIN-4 fills content without re-plumbing the face.
    #[allow(
        clippy::unused_self,
        clippy::missing_const_for_fn,
        reason = "the CURTAIN-4 extension seam: kept as a plain method so the fill \
                  reaches the curtain's state without re-plumbing the face"
    )]
    fn face_extras(&self, _ui: &mut egui::Ui) {}

    /// The password stage (lock 8): the low-centre field (masked input), the
    /// verify spinner, the honest deny line, or the backoff wall's live
    /// countdown. Enter submits to the [`Verifier`] seam; the field re-takes
    /// focus every frame while it accepts input.
    fn password_stage(&mut self, ui: &mut egui::Ui) {
        let enabled = self.stage_accepts_input();
        let field = egui::TextEdit::singleline(&mut self.password)
            .id(egui::Id::new(PASSWORD_FIELD))
            .password(true)
            .hint_text("Password")
            .horizontal_align(egui::Align::Center)
            .desired_width(FIELD_W);
        let response = ui.add_enabled(enabled, field);
        if enabled {
            // The canonical egui submit: Enter surrenders the field's focus.
            let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if submitted && !self.password.is_empty() {
                self.submit();
            } else {
                // The stage owns the keyboard while it accepts input (lock 10).
                response.request_focus();
            }
        }

        ui.add_space(Style::SP_S);
        if matches!(self.phase, Phase::Verifying) {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(Style::BODY).color(Style::ACCENT));
                ui.label(
                    RichText::new("Verifying…")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
            });
        } else if let Some(remaining) = self.backoff_remaining() {
            ui.label(
                RichText::new(format!(
                    "Too many attempts — try again in {:.0}s",
                    remaining.max(0.0).ceil()
                ))
                .color(Style::WARN)
                .size(Style::SMALL),
            );
        } else if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(Style::DANGER).size(Style::SMALL));
        } else {
            ui.label(
                RichText::new("Enter to unlock · Esc to dismiss")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
    }
}

// ──────────────────────────────────── tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::MeshSummary;
    use crate::dock::{self, Surface};
    use crate::tray::{TrayInputs, TrayState};
    use std::collections::VecDeque;

    // ── the scripted verify seam ──

    /// A scripted [`Verifier`]: `begin` counts attempts, `poll` pops the next
    /// scripted verdict — the exact seam CURTAIN-2's PAM conversation fills.
    struct Scripted {
        verdicts: VecDeque<Verdict>,
        began: usize,
    }

    impl Scripted {
        fn denies(n: usize) -> Box<dyn Verifier> {
            Box::new(Self {
                verdicts: (0..n)
                    .map(|_| Verdict::Denied("wrong password".to_owned()))
                    .collect(),
                began: 0,
            })
        }

        fn grants() -> Box<dyn Verifier> {
            Box::new(Self {
                verdicts: VecDeque::from(vec![Verdict::Granted]),
                began: 0,
            })
        }
    }

    impl Verifier for Scripted {
        fn begin(&mut self, _password: &str) {
            self.began += 1;
        }
        fn poll(&mut self) -> Option<Verdict> {
            self.verdicts.pop_front()
        }
    }

    // ── pure-machine helpers ──

    /// Tick `secs` through in ≤ 50 ms sub-steps (the motion clamp caps a
    /// single step, so a long span must be walked; an integer loop keeps the
    /// float bookkeeping exact).
    fn step(c: &mut Curtain, secs: f32) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "test helper: secs is a small positive span"
        )]
        let n = ((secs / 0.05).ceil() as u32).max(1);
        #[allow(
            clippy::cast_precision_loss,
            reason = "n is tiny — far below f32's exact-integer range"
        )]
        let dt = secs / n as f32;
        for _ in 0..n {
            c.tick(dt);
        }
    }

    /// A curtain settled onto the locked face, over the given seam.
    fn locked(verifier: Box<dyn Verifier>) -> Curtain {
        let mut c = Curtain::with_verifier(verifier);
        c.lock();
        step(&mut c, DROP_SECS + 0.1);
        assert!(
            matches!(c.phase, Phase::Locked { .. }),
            "the drop did not settle locked"
        );
        c
    }

    /// One full attempt: type, submit, and tick once so the verdict lands.
    fn attempt(c: &mut Curtain, password: &str) {
        c.password.push_str(password);
        c.submit();
        c.tick(0.016);
    }

    /// One attempt through an OFF-THREAD verifier (the real [`PamVerifier`]):
    /// submit, then tick in bounded sub-steps until the worker's verdict lands
    /// and the machine leaves `Verifying` (unlike `Scripted`, whose verdict is
    /// queued synchronously in `begin`).
    fn attempt_async(c: &mut Curtain, password: &str) {
        c.password.push_str(password);
        c.submit();
        assert!(matches!(c.phase, Phase::Verifying), "submit must enter Verifying");
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(c.phase, Phase::Verifying) {
            c.tick(0.01);
            assert!(Instant::now() < deadline, "the verdict never landed off-thread");
            std::thread::yield_now();
        }
    }

    fn key(k: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 720.0),
            )),
            events,
            ..Default::default()
        }
    }

    // ── the state machine ──

    #[test]
    fn lock_drops_the_curtain_and_settles_into_the_locked_face() {
        let mut c = Curtain::default();
        assert!(!c.engaged());
        assert!(
            (c.offset_fraction() + 1.0).abs() < 1e-6,
            "an unlocked curtain rests fully above the screen"
        );

        c.lock();
        assert!(matches!(c.phase, Phase::Dropping { .. }));
        assert!(
            c.engaged(),
            "input exclusivity begins the instant the drop starts"
        );
        assert!(!c.covers_fully(), "still sliding — the view shows beneath");

        c.tick(DROP_SECS / 2.0);
        assert!(matches!(c.phase, Phase::Dropping { .. }));

        c.tick(DROP_SECS);
        assert!(matches!(c.phase, Phase::Locked { .. }));
        assert!(c.covers_fully());
        assert!(
            c.offset_fraction().abs() < f32::EPSILON,
            "the settled sheet covers exactly (offset 0)"
        );

        // lock() while engaged is idempotent — no re-drop from the face.
        c.lock();
        assert!(matches!(c.phase, Phase::Locked { .. }));
    }

    #[test]
    fn any_key_reveals_and_escape_drops_back_to_the_face() {
        let mut c = locked(Scripted::denies(0));
        c.fold_input(Signals {
            wake: true,
            reveal: true,
            escape: false,
        });
        assert!(matches!(c.phase, Phase::Revealing));
        assert!(c.stage_accepts_input());

        // An empty buffer never submits — the stage stays up, nothing verifies.
        c.submit();
        assert!(matches!(c.phase, Phase::Revealing));

        c.password.push_str("half-typed");
        c.fold_input(Signals {
            wake: true,
            reveal: true,
            escape: true,
        });
        assert!(
            matches!(c.phase, Phase::Locked { .. }),
            "Esc returns to the face"
        );
        assert!(
            c.password.is_empty(),
            "the buffer never survives the dismissal"
        );
    }

    #[test]
    fn a_deny_returns_an_honest_error_and_five_arm_the_backoff_wall() {
        let mut c = locked(Scripted::denies(5));
        c.reveal();
        for i in 1..=4 {
            attempt(&mut c, "wrong");
            assert!(
                matches!(c.phase, Phase::Revealing),
                "deny {i} must return to the password stage"
            );
            assert_eq!(c.error.as_deref(), Some("wrong password"));
            assert_eq!(c.fails, i);
            assert!(c.password.is_empty(), "the buffer clears on submit");
        }

        attempt(&mut c, "wrong");
        let remaining = c
            .backoff_remaining()
            .expect("the 5th deny must arm the backoff wall");
        assert!((remaining - BACKOFF_SECS).abs() < 1e-3);
        assert!(
            !c.stage_accepts_input(),
            "the field is disabled behind the wall"
        );
        assert!(c.engaged() && c.covers_fully());

        // The countdown is live…
        c.tick(1.0);
        let remaining = c.backoff_remaining().expect("still cooling down");
        assert!((remaining - (BACKOFF_SECS - 1.0)).abs() < 1e-3);

        // …and the wall falls after the full cooldown, to a fresh window.
        step(&mut c, BACKOFF_SECS);
        assert!(matches!(c.phase, Phase::Revealing));
        assert_eq!(c.fails, 0, "the attempt window resets after the cooldown");
        assert!(c.stage_accepts_input());
    }

    #[test]
    fn the_pam_verifier_seam_denies_off_thread_and_five_arm_the_backoff_wall() {
        use crate::pam_auth::PamVerifier;
        // The REAL CURTAIN-2 verifier — its genuine off-thread channel bridge —
        // over a scripted deny-only backend (a unit test NEVER runs real PAM).
        // Confirms 5 real denials drive the existing 30s cooldown (lock 10).
        let deny = std::sync::Arc::new(|_user: &str, _password: &str| {
            Verdict::Denied("incorrect password".to_owned())
        });
        let verifier = PamVerifier::with_backend(Some("seat".to_owned()), deny);
        let mut c = locked(Box::new(verifier));
        c.reveal();

        for i in 1..=4 {
            attempt_async(&mut c, "wrong");
            assert!(
                matches!(c.phase, Phase::Revealing),
                "deny {i} must return to the password stage"
            );
            assert_eq!(c.fails, i);
            assert!(c.password.is_empty(), "the buffer clears on submit");
        }

        attempt_async(&mut c, "wrong");
        let remaining = c
            .backoff_remaining()
            .expect("the 5th real denial must arm the backoff wall");
        assert!((remaining - BACKOFF_SECS).abs() < 1e-3);
        assert!(!c.stage_accepts_input(), "the field disables behind the wall");
        assert!(c.engaged() && c.covers_fully());
    }

    #[test]
    fn a_granted_verdict_lifts_the_curtain_through_the_seam() {
        let mut c = locked(Scripted::grants());
        c.reveal();
        c.password.push_str("correct-horse");
        c.submit();
        assert!(matches!(c.phase, Phase::Verifying));
        assert!(c.password.is_empty(), "the buffer clears on submit");

        c.tick(0.016);
        assert!(
            matches!(c.phase, Phase::Lifting { .. }),
            "a grant starts the lift"
        );
        assert!(c.engaged(), "still input-exclusive while lifting");
        assert!(
            !c.covers_fully(),
            "the view returns beneath the rising sheet"
        );

        step(&mut c, LIFT_SECS + 0.1);
        assert!(!c.engaged(), "the lift ends unlocked");
    }

    #[test]
    fn the_default_verifier_is_forbidden_from_unlocking() {
        // CURTAIN-1 ships no authenticator: the default seam denies every
        // attempt, honestly, and can never produce a lift.
        let mut c = Curtain::default();
        c.lock();
        step(&mut c, DROP_SECS + 0.1);
        c.reveal();
        c.password.push_str("any password at all");
        c.submit();
        for _ in 0..200 {
            c.tick(0.05);
            assert!(c.engaged(), "the not-wired curtain must NEVER unlock");
            assert!(!matches!(c.phase, Phase::Lifting { .. }));
        }
        assert!(matches!(c.phase, Phase::Revealing));
        let error = c.error.as_deref().expect("an honest deny line");
        assert!(error.contains("CURTAIN-2"), "the deny says why: {error}");
    }

    #[test]
    fn the_face_dims_after_idle_and_any_input_wakes_it() {
        let mut c = locked(Scripted::denies(0));
        c.tick(IDLE_DIM_SECS - 1.0);
        assert!(!c.is_dimmed(), "dimmed before the idle threshold");
        c.tick(1.5);
        assert!(c.is_dimmed(), "the face dims past ~30 s idle");

        // Any activity (a pointer drift is enough) wakes the face.
        c.fold_input(Signals {
            wake: true,
            reveal: false,
            escape: false,
        });
        assert!(!c.is_dimmed(), "input must wake the dimmed face");

        // A key on the (re-)dimmed face wakes AND reveals.
        c.tick(IDLE_DIM_SECS + 1.0);
        assert!(c.is_dimmed());
        c.fold_input(Signals {
            wake: true,
            reveal: true,
            escape: false,
        });
        assert!(matches!(c.phase, Phase::Revealing));
    }

    // ── motion (lock 5) ──

    #[test]
    fn the_drop_overshoots_past_the_bottom_and_settles_at_zero() {
        assert!(
            (drop_offset(0.0) + 1.0).abs() < 1e-4,
            "the drop starts fully above ({})",
            drop_offset(0.0)
        );
        assert!(
            drop_offset(1.0).abs() < 1e-6,
            "the drop must settle at exactly 0 ({})",
            drop_offset(1.0)
        );
        // The settle bounce: somewhere mid-flight the sheet passes the bottom.
        let max = (0..=100_u8)
            .map(|i| drop_offset(f32::from(i) / 100.0))
            .fold(f32::MIN, f32::max);
        assert!(max > 0.02, "no settle overshoot: max offset {max}");
        assert!(
            max < OVERSHOOT_BLEED,
            "the overshoot ({max}) escapes the paint bleed ({OVERSHOOT_BLEED})"
        );

        // The lift leaves from covering and exits fully above.
        assert!(lift_offset(0.0).abs() < 1e-6);
        assert!((lift_offset(1.0) + 1.0).abs() < 1e-6);
        assert!(
            lift_offset(0.5) > -0.5,
            "the lift eases in — slow peel, fast exit"
        );
    }

    // ── the clock face (lock 6) ──

    #[test]
    fn face_lines_stack_hh_mm_over_the_civil_date() {
        assert_eq!(face_lines(0), ("00:00".to_owned(), "1970-01-01".to_owned()));
        // The same vector the tray's stacked clock asserts — one calendar (§6).
        assert_eq!(
            face_lines(1_577_836_800 + 13 * 3600 + 5 * 60 + 59),
            ("13:05".to_owned(), "2020-01-01".to_owned())
        );
    }

    // ── input folding ──

    #[test]
    fn raw_input_folds_to_wake_reveal_and_escape() {
        // A pointer drift only wakes (never a phantom reveal).
        let s = fold_events(&[egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]);
        assert_eq!(
            s,
            Signals {
                wake: true,
                reveal: false,
                escape: false
            }
        );
        // A key press wakes AND reveals; a release is nothing.
        let s = fold_events(&[key(egui::Key::A, true)]);
        assert_eq!(
            s,
            Signals {
                wake: true,
                reveal: true,
                escape: false
            }
        );
        assert_eq!(fold_events(&[key(egui::Key::A, false)]), Signals::default());
        // A click reveals.
        let s = fold_events(&[egui::Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }]);
        assert!(s.reveal && s.wake && !s.escape);
        // Escape carries the escape signal.
        let s = fold_events(&[key(egui::Key::Escape, true)]);
        assert!(s.escape && s.wake);
        // A non-user event is nothing — it must not wake the dim.
        assert_eq!(fold_events(&[egui::Event::PointerGone]), Signals::default());
    }

    // ── input exclusivity (lock 10) ──

    /// Mount the shell's real bottom taskbar exactly as `render` does, plus —
    /// optionally — the engaged curtain drawn after it (the render order in
    /// `main.rs`), for one headless frame.
    fn shell_frame(
        ctx: &egui::Context,
        active: &mut Surface,
        mut curtain: Option<&mut Curtain>,
        events: Vec<egui::Event>,
    ) {
        let mesh = MeshSummary::default();
        let inputs = TrayInputs {
            mesh: &mesh,
            seat: None,
            unread: 0,
            session_active: false,
        };
        let mut tray = TrayState::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::TopBottomPanel::bottom("shell-taskbar")
                .exact_height(dock::TASKBAR_H)
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| {
                    let _ = dock::taskbar(ui, active, &mut tray, &inputs);
                });
            // Reborrow per frame — `ctx.run`'s closure is `FnMut`.
            if let Some(c) = curtain.as_deref_mut() {
                c.show(ctx);
            }
        });
    }

    #[test]
    fn the_engaged_curtain_swallows_clicks_before_the_dock() {
        // The dock's leftmost (Workbench) cell centre — derived from the same
        // layout tokens as dock's private CELL_W (`SP_XL + SP_M`; dock.rs is
        // read-only for CURTAIN-1, so the constant is restated here).
        let cell_w = Style::SP_XL + Style::SP_M;
        let click = egui::pos2(cell_w / 2.0, 600.0 - dock::TASKBAR_H / 2.0);
        let press = egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };

        // Control: with no curtain, the exact same click sequence routes — so
        // a pass below can only mean the curtain swallowed it.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::About;
        shell_frame(&ctx, &mut active, None, Vec::new());
        shell_frame(&ctx, &mut active, None, Vec::new());
        shell_frame(
            &ctx,
            &mut active,
            None,
            vec![egui::Event::PointerMoved(click), press.clone()],
        );
        shell_frame(&ctx, &mut active, None, vec![release.clone()]);
        assert_eq!(
            active,
            Surface::Workbench,
            "control: with no curtain the click must select Workbench"
        );

        // The engaged curtain claims the whole screen (taskbar included):
        // the identical click never reaches the dock (lock 10).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut curtain = Curtain::default();
        curtain.lock();
        step(&mut curtain, DROP_SECS + 0.1); // settled onto the face
        let mut active = Surface::About;
        shell_frame(&ctx, &mut active, Some(&mut curtain), Vec::new());
        shell_frame(&ctx, &mut active, Some(&mut curtain), Vec::new());
        shell_frame(
            &ctx,
            &mut active,
            Some(&mut curtain),
            vec![egui::Event::PointerMoved(click), press],
        );
        shell_frame(&ctx, &mut active, Some(&mut curtain), vec![release]);
        assert_eq!(
            active,
            Surface::About,
            "a click under the curtain reached the dock (lock 10)"
        );
        assert!(curtain.engaged(), "the curtain must still stand");
    }

    #[test]
    fn the_curtain_steals_focus_and_its_password_field_takes_it() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = Curtain::default();
        c.lock();
        step(&mut c, DROP_SECS + 0.1);

        // A surface field beneath holds keyboard focus as the curtain drops…
        let beneath = egui::Id::new("some-surface-field");
        ctx.memory_mut(|m| m.request_focus(beneath));
        let _ = ctx.run(raw(Vec::new()), |ctx| c.show(ctx));
        assert_ne!(
            ctx.memory(egui::Memory::focused),
            Some(beneath),
            "keyboard focus must not stay under the curtain (lock 10)"
        );

        // …any key reveals the stage, whose field takes the keyboard.
        let _ = ctx.run(raw(vec![key(egui::Key::A, true)]), |ctx| c.show(ctx));
        assert!(matches!(c.phase, Phase::Revealing));
        let _ = ctx.run(raw(Vec::new()), |ctx| c.show(ctx));
        assert_eq!(
            ctx.memory(egui::Memory::focused),
            Some(egui::Id::new(PASSWORD_FIELD)),
            "the curtain's own field owns the keyboard while revealing"
        );
    }

    // ── rendering ──

    /// Count the text shapes in a frame's output, recursing into groups.
    fn count_text_shapes(shape: &egui::Shape, n: &mut usize) {
        match shape {
            egui::Shape::Text(_) => *n += 1,
            egui::Shape::Vec(v) => {
                for s in v {
                    count_text_shapes(s, n);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_locked_face_paints_the_giant_clock_over_an_opaque_sheet() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = locked(Scripted::denies(0));
        // Prime one frame — egui sizes a fresh Area before its first real paint.
        let _ = ctx.run(raw(Vec::new()), |ctx| c.show(ctx));
        let out = ctx.run(raw(Vec::new()), |ctx| c.show(ctx));
        let mut texts = 0;
        for clipped in &out.shapes {
            count_text_shapes(&clipped.shape, &mut texts);
        }
        assert_eq!(
            texts, 2,
            "the resting face carries exactly the clock's two lines (HH:MM + date)"
        );
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the curtain painted no draw primitives");
    }

    #[test]
    fn an_unlocked_curtain_paints_and_claims_nothing() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = Curtain::default();
        let out = ctx.run(raw(Vec::new()), |ctx| c.show(ctx));
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            prims.is_empty(),
            "an unlocked curtain must draw nothing at all"
        );
    }
}
