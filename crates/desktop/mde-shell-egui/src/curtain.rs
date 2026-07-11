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
//! scripted verdict.
//!
//! **CURTAIN-4** fills [`Curtain::face_extras`]: the unified now-playing strip
//! (the active media player's title + play/pause/next/prev, driven through the
//! [`LockMedia`] seam) and a master-volume slider (the [`LockMixer`] seam over
//! the same `wpctl` backend the tray drives) work WHILE locked — design lock 3,
//! playback needs no unlock — and are the ONLY input the engaged curtain exempts
//! from its whole-screen grab (lock 10). Beside them the status row glances
//! battery / mesh health / date (lock 4); no message content ever reaches the
//! curtain (lock 4 — chat stays private until unlock). Music is honestly scoped
//! out: `mde-music-egui` exposes no in-process transport seam to drive.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, Splash, …); the shell in main.rs consumes them"
)]

use std::time::{Duration, Instant};

use mde_egui::egui::{self, Align2, Color32, FontId, RichText};
use mde_egui::{Motion, Style};

use mde_lighthouse_health::LighthouseHealth;
use mde_media_egui::{MediaSurface, TransportAction};
use mde_seat::{Battery, BatteryState, MixerClient, PwGraph, SeatError, SeatSnapshot};
use mde_theme::brand::icons::IconId;

use crate::chrome::MeshSummary;
use crate::dock::icon_texture;

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

// ─────────────────────── the CURTAIN-4 transport seams ───────────────────────

/// The now-playing view the curtain's locked strip renders — the active media
/// player's title and whether it is playing. An owned snapshot (not a borrow of
/// the player) so the seam stays render-agnostic and the tests can script it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NowPlaying {
    /// The loaded track's display title (the media surface's own now-playing
    /// title fold — a file stem or a stream title, never invented).
    title: String,
    /// Whether the engine is actively playing (paused/idle reads `false`).
    playing: bool,
}

/// The **media transport seam** (design lock 3/7): the curtain drives whichever
/// player the shell holds without reaching into it. Production wires it to the
/// live [`MediaSurface`] (`self.media` in the shell — the ONE media player the
/// shell owns); tests wire a recording fake to assert the locked strip drives
/// the real transport. Music is deliberately absent — `mde-music-egui` exposes
/// no in-process transport seam, so the curtain honestly drives media only.
pub(crate) trait LockMedia {
    /// The active track, or `None` when nothing is loaded (the honest "nothing
    /// playing" state — the strip then shows no dead controls).
    fn now_playing(&self) -> Option<NowPlaying>;
    /// Toggle play/pause on the live player.
    fn toggle_play(&mut self);
    /// Advance to the next queued track.
    fn next(&mut self);
    /// Step back to the previous queued track.
    fn prev(&mut self);
}

/// The live wiring: the shell's production media surface drives the strip
/// directly (glue over its public transport — [`MediaController::dispatch`] +
/// the now-playing fold — no reimplementation, §6).
impl LockMedia for MediaSurface {
    fn now_playing(&self) -> Option<NowPlaying> {
        // `media()` is `Some` only while a track is loaded (a Stop unloads it),
        // so this is the honest "something is playing/paused" gate.
        self.player().media().map(|_| NowPlaying {
            title: mde_media_egui::model::now_playing_title(self.player()),
            playing: self.is_playing(),
        })
    }

    fn toggle_play(&mut self) {
        self.dispatch(TransportAction::TogglePlay);
    }

    fn next(&mut self) {
        self.dispatch(TransportAction::Next);
    }

    fn prev(&mut self) {
        self.dispatch(TransportAction::Prev);
    }
}

/// The **master-volume seam** (design lock 3): the curtain's volume slider +
/// mute toggle drive the SAME host mixer (`wpctl`/`PipeWire`) the System surface's
/// mixer controls do — the master output is a global host service, so the curtain
/// owns its own lazy client rather than reaching the System state's `Seat`
/// handle (lock 1). Tests inject a recording fake. The current level is read
/// from the passed [`SeatSnapshot`], not this seam — a write-only verb pair.
trait LockMixer {
    /// Set the master strip's volume (0–100).
    ///
    /// # Errors
    /// The mixer client's typed error (absent `PipeWire` → `Unavailable`).
    fn set_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError>;

    /// Set the master strip's mute.
    ///
    /// # Errors
    /// As [`Self::set_volume`].
    fn set_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError>;
}

/// The production mixer verbs: `PipeWire` via `wpctl`, the same backend the seat
/// snapshot the curtain reads is folded from. Lazy — no I/O until the slider is
/// actually moved, so a headless / never-locked curtain never touches the host.
struct HostMixer {
    /// The `PipeWire` graph client (volume/mute via `wpctl`).
    mixer: PwGraph,
}

impl HostMixer {
    /// Wire the live host client (no I/O here).
    fn new() -> Self {
        Self {
            mixer: PwGraph::new(),
        }
    }
}

impl LockMixer for HostMixer {
    fn set_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError> {
        self.mixer.set_volume(strip_id, volume)
    }

    fn set_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError> {
        self.mixer.set_muted(strip_id, muted)
    }
}

/// A confirmed master-mixer write, echoed over the seat snapshot until the
/// System state's ~5s poll catches up — the tray's echo pattern, one slot (the
/// master strip). Set ONLY on a verb's `Ok` (§7 — a refused write never
/// pretends), so the slider + mute glyph read the just-written value back
/// instantly instead of snapping to the stale snapshot between polls.
#[derive(Debug, Clone)]
struct VolEcho {
    /// The master strip id the write targeted.
    id: String,
    /// The confirmed volume (0–100).
    volume: u8,
    /// The confirmed mute.
    muted: bool,
    /// When the write landed (the TTL clock).
    at: Instant,
}

/// The master strip the volume slider renders + drives: its id, the
/// echo-folded volume, and the echo-folded mute.
struct MasterView {
    /// The strip id the verbs key on.
    id: String,
    /// The effective volume (echo over snapshot).
    volume: u8,
    /// The effective mute (echo over snapshot).
    muted: bool,
}

/// How long a confirmed mixer echo may outlive the snapshot — one ~5s seat poll
/// plus slack, matching the tray so the two never disagree on the master.
const VOL_ECHO_TTL: Duration = Duration::from_secs(8);

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
const EXTRAS_Y_FRAC: f32 = 0.52;
/// The password stage's centre line (low-centre, lock 8).
const FIELD_Y_FRAC: f32 = 0.78;
/// The extras region's width as a fraction of the sheet width.
const EXTRAS_W_FRAC: f32 = 0.5;
/// The extras region's height as a fraction of the sheet height — tall enough
/// for the now-playing strip, the volume row, and the status glanceables.
const EXTRAS_H_FRAC: f32 = 0.22;
/// The locked volume slider's width, on the spacing grid.
const VOL_SLIDER_W: f32 = Style::SP_XL * 5.0;
/// The mute-toggle glyph edge (the compact status raster idiom).
const GLANCE_ICON: f32 = Style::SP_M;
/// A status-row glanceable dot's radius (the compact state-dot idiom).
const GLANCE_DOT_R: f32 = Style::SP_XS / 2.0;
/// Charge (%) at/under which the battery glanceable reads red, and under which
/// it reads amber (the status ladder, restated for the lock screen).
const GLANCE_BATTERY_CRITICAL: f64 = 5.0;
const GLANCE_BATTERY_LOW: f64 = 20.0;
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
/// (or wheel/zoom/touch drift) only wakes the dim; a key press, text, or paste
/// reveals the stage; releases and non-user events (a focus flip, `PointerGone`)
/// are nothing — never a phantom reveal.
///
/// A pointer press reveals the stage **unless** it lands inside `exempt` — the
/// now-playing / volume strip's hit area (CURTAIN-4, design lock 3): a click
/// there drives the transport, not an unlock, so it only wakes. That strip is
/// the ONLY interactive region the engaged curtain exempts besides the field.
fn fold_events(events: &[egui::Event], exempt: Option<egui::Rect>) -> Signals {
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
            | egui::Event::Paste(_) => {
                s.reveal = true;
                s.wake = true;
            }
            egui::Event::PointerButton {
                pressed: true, pos, ..
            } => {
                s.wake = true;
                if !exempt.is_some_and(|r| r.contains(*pos)) {
                    s.reveal = true;
                }
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
/// runs ([`crate::chat::civil_from_days`] is the crate's ONE calendar, §6),
/// restated here rather than reaching across surface modules.
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
/// opaque** — a dimmed curtain must never become a window onto the desktop. The
/// opaque channel-fold lives in the shared kit ([`Style::scale_rgb_opaque`]) so
/// no colour is minted here (§4).
fn near_black(c: Color32, dim: f32) -> Color32 {
    Style::scale_rgb_opaque(c, dim.mul_add(-BG_DROP, 1.0))
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
    /// The CURTAIN-4 master-volume seam (real `wpctl` by default; a recording
    /// fake in tests). Write-only — the live level is read from the seat
    /// snapshot the shell hands [`Curtain::show`], never from here.
    mixer: Box<dyn LockMixer>,
    /// The in-flight master-mixer write echo (CURTAIN-4), bridging the ~5s
    /// seat-poll gap so a just-driven volume/mute reads back instantly.
    vol_echo: Option<VolEcho>,
    /// The last refused mixer verb's honest message (§7), shown under the
    /// locked volume slider; cleared by the next successful write.
    mixer_error: Option<String>,
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
            mixer: Box::new(HostMixer::new()),
            vol_echo: None,
            mixer_error: None,
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
    ///
    /// CURTAIN-4 hands in the live transport + status state the locked face
    /// exposes: `media` is the active player the now-playing strip drives
    /// (design lock 3/7), `seat` carries the master-mixer level + the battery
    /// glanceable (lock 4), and `mesh` the network-health glanceable (lock 4).
    /// The shell passes its own `self.media` / `system.snapshot()` /
    /// `chrome.summary()` — the same reads the dock status folds, no new poll.
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        media: &mut dyn LockMedia,
        seat: Option<&SeatSnapshot>,
        mesh: &MeshSummary,
    ) {
        if !self.engaged() {
            self.last_frame = None;
            return;
        }

        // 1 — this frame's raw input, folded to the curtain's typed signals.
        // The now-playing / volume strip's hit area is exempt from the reveal
        // trigger (design lock 3) — but only while it actually carries a live
        // control: an idle, player-less, mixer-less strip has nothing to drive,
        // so a press there reveals the stage like anywhere else on the face.
        let exempt = (matches!(self.phase, Phase::Locked { .. })
            && extras_interactive(&*media, seat))
        .then(|| extras_rect(ctx.screen_rect()));
        let signals = ctx.input(|i| fold_events(&i.events, exempt));
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
                self.paint_sheet(ui, sheet, dim, media, seat, mesh);
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
    /// the overshoot bleed, the giant clock face, the CURTAIN-4 extras seam
    /// (now-playing strip + volume + status glanceables), and the two-stage
    /// password reveal.
    fn paint_sheet(
        &mut self,
        ui: &mut egui::Ui,
        sheet: egui::Rect,
        dim: f32,
        media: &mut dyn LockMedia,
        seat: Option<&SeatSnapshot>,
        mesh: &MeshSummary,
    ) {
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

        // The CURTAIN-4 seam: the unified now-playing strip + the volume row +
        // the glanceable status row mount in this region (between the date line
        // and the password stage). The idle dim fades them with the face.
        let mut extras_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(extras_rect(sheet))
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        self.face_extras(&mut extras_ui, dim, media, seat, mesh);

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
    /// unified now-playing transport strip (design lock 3/7), the master-volume
    /// row (lock 3), and the status glanceables (battery / mesh / date, lock 4).
    /// All three fade with the idle dim. No message content is ever shown — the
    /// curtain is handed no chat state to leak (lock 4).
    fn face_extras(
        &mut self,
        ui: &mut egui::Ui,
        dim: f32,
        media: &mut dyn LockMedia,
        seat: Option<&SeatSnapshot>,
        mesh: &MeshSummary,
    ) {
        // The extras fade with the face's idle dim — faint, never gone (lock 10).
        ui.set_opacity(dim.mul_add(-FAINT_DROP, 1.0));
        let now = Instant::now();

        now_playing_strip(ui, media);
        ui.add_space(Style::SP_S);
        self.volume_row(ui, seat, now);
        ui.add_space(Style::SP_S);
        status_row(ui, seat, mesh);
    }

    /// The master-volume row (design lock 3): the mute-toggle speaker glyph + a
    /// slider driving the REAL host mixer through the [`LockMixer`] seam, over
    /// the echo-folded master level. An absent mixer renders the honest
    /// not-available line instead of a dead slider (§7); a refused write
    /// surfaces its typed error underneath.
    fn volume_row(&mut self, ui: &mut egui::Ui, seat: Option<&SeatSnapshot>, now: Instant) {
        self.reconcile_vol_echo(seat, now);
        let Some(master) = self.master_view(seat, now) else {
            mde_egui::muted_note(ui, "Volume unavailable");
            return;
        };
        let mut level = master.volume;
        ui.horizontal(|ui| {
            if mute_button(ui, master.muted) {
                self.drive_mute(&master.id, !master.muted, master.volume, now);
            }
            ui.spacing_mut().slider_width = VOL_SLIDER_W;
            if ui
                .add(egui::Slider::new(&mut level, 0..=100).show_value(false))
                .changed()
            {
                self.drive_volume(&master.id, level, master.muted, now);
            }
        });
        if let Some(e) = &self.mixer_error {
            ui.label(RichText::new(e).size(Style::SMALL).color(Style::DANGER));
        }
    }

    /// Drop the volume echo once the seat snapshot reflects it (the ~5s poll
    /// caught up) or it outlived [`VOL_ECHO_TTL`] — past that, the real state
    /// wins again (a change made from the System surface must not be overridden
    /// forever by a stale echo).
    fn reconcile_vol_echo(&mut self, seat: Option<&SeatSnapshot>, now: Instant) {
        if let Some(e) = &self.vol_echo {
            let caught_up = seat.and_then(|s| s.mixer.present()).is_some_and(|m| {
                m.master.id == e.id && m.master.volume == e.volume && m.master.muted == e.muted
            });
            if caught_up || now.duration_since(e.at) >= VOL_ECHO_TTL {
                self.vol_echo = None;
            }
        }
    }

    /// The master strip to render + drive: the snapshot's master with any live
    /// echo folded over it. `None` when the seat carries no mixer.
    fn master_view(&self, seat: Option<&SeatSnapshot>, now: Instant) -> Option<MasterView> {
        let master = &seat?.mixer.present()?.master;
        if let Some(e) = &self.vol_echo {
            if e.id == master.id && now.duration_since(e.at) < VOL_ECHO_TTL {
                return Some(MasterView {
                    id: master.id.clone(),
                    volume: e.volume,
                    muted: e.muted,
                });
            }
        }
        Some(MasterView {
            id: master.id.clone(),
            volume: master.volume,
            muted: master.muted,
        })
    }

    /// Drive the master volume through the seam: on `Ok`, echo it so the slider
    /// reads back instantly; on a refusal, surface the typed error and echo
    /// NOTHING (§7 — the stale snapshot stays the truth).
    fn drive_volume(&mut self, id: &str, volume: u8, muted: bool, now: Instant) {
        match self.mixer.set_volume(id, volume) {
            Ok(()) => {
                self.mixer_error = None;
                self.vol_echo = Some(VolEcho {
                    id: id.to_owned(),
                    volume,
                    muted,
                    at: now,
                });
            }
            Err(e) => self.mixer_error = Some(format!("volume: {e}")),
        }
    }

    /// Drive the master mute — the same confirm-then-echo contract; the echo is
    /// what flips the speaker glyph live.
    fn drive_mute(&mut self, id: &str, muted: bool, volume: u8, now: Instant) {
        match self.mixer.set_muted(id, muted) {
            Ok(()) => {
                self.mixer_error = None;
                self.vol_echo = Some(VolEcho {
                    id: id.to_owned(),
                    volume,
                    muted,
                    at: now,
                });
            }
            Err(e) => self.mixer_error = Some(format!("mute: {e}")),
        }
    }

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

// ─────────────────────── the CURTAIN-4 face-extras folds ──────────────────────

/// The CURTAIN-4 extras region's rect within a `sheet` — the now-playing strip,
/// the volume row, and the status glanceables mount here (between the date line
/// and the password stage). Shared by the painter AND the input-exemption fold
/// so the hit area exempted from the reveal is exactly what's drawn (lock 3).
fn extras_rect(sheet: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(
        egui::pos2(
            sheet.center().x,
            sheet.height().mul_add(EXTRAS_Y_FRAC, sheet.top()),
        ),
        egui::vec2(
            sheet.width() * EXTRAS_W_FRAC,
            sheet.height() * EXTRAS_H_FRAC,
        ),
    )
}

/// Whether the extras strip carries a live control this frame — a loaded player
/// (play/pause/next/prev) or a present mixer (the volume slider). Only then is
/// its hit area worth exempting from the reveal trigger: an inert strip has
/// nothing to drive, so a press there should reveal the stage (design lock 3).
fn extras_interactive(media: &dyn LockMedia, seat: Option<&SeatSnapshot>) -> bool {
    media.now_playing().is_some() || seat.is_some_and(|s| s.mixer.present().is_some())
}

/// The unified now-playing strip (design lock 3/7): the active player's title +
/// a live state word over a prev · play/pause · next transport, driving the
/// [`LockMedia`] seam. With nothing loaded it shows the honest "Nothing playing"
/// line and NO controls — never a dead button (§7).
fn now_playing_strip(ui: &mut egui::Ui, media: &mut dyn LockMedia) {
    let Some(np) = media.now_playing() else {
        mde_egui::muted_note(ui, "Nothing playing");
        return;
    };
    let playing = np.playing;
    ui.label(
        RichText::new(np.title)
            .size(Style::BODY)
            .color(Style::ACCENT),
    );
    ui.label(
        RichText::new(if playing { "Now playing" } else { "Paused" })
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        if ui.button("Prev").clicked() {
            media.prev();
        }
        if ui.button(if playing { "Pause" } else { "Play" }).clicked() {
            media.toggle_play();
        }
        if ui.button("Next").clicked() {
            media.next();
        }
    });
}

/// The glanceable status row (design lock 4): battery · mesh health · date — the
/// ONLY status the curtain shows. NO message content: chat previews stay private
/// until unlock, and the curtain is handed no chat state to leak. The status fold
/// logic is restated locally for the lock screen.
fn status_row(ui: &mut egui::Ui, seat: Option<&SeatSnapshot>, mesh: &MeshSummary) {
    ui.horizontal(|ui| {
        if let Some((pct, tone)) = battery_glance(seat) {
            glance_dot(ui, tone);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a battery percentage is a clamped 0..=100 value"
            )]
            let label = format!("{}%", pct.round() as u32);
            ui.label(
                RichText::new(label)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_M);
        }
        let (tone, word) = mesh_glance(mesh);
        glance_dot(ui, tone);
        ui.label(
            RichText::new(word)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_M);
        let (_, date) = face_lines(unix_now());
        ui.label(
            RichText::new(date)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    });
}

/// A tiny tone dot (the tray's at-a-glance state idiom) leading a glanceable.
fn glance_dot(ui: &mut egui::Ui, tone: Color32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(Style::SP_S, Style::SP_S), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), GLANCE_DOT_R, tone);
}

/// The mute-toggle speaker glyph (the tray's volume-flyout affordance): the
/// muted variant + a WARN tint while muted, hover fill, no tooltip. Returns
/// `true` on a click.
///
/// The interactive cell is a comfortable [`Style::SP_XL`] (32px) square — the
/// same edge the dock's picker cells (`APP_CELL_H`) sit on — so the lock face's
/// mute target clears the pointer/touch minimum; the speaker glyph stays its
/// glanceable [`GLANCE_ICON`] size, centred inside the larger hit rect.
fn mute_button(ui: &mut egui::Ui, muted: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(Style::SP_XL, Style::SP_XL), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let (glyph, tint) = if muted {
        (IconId::VolumeMuted, Style::WARN)
    } else {
        (IconId::Volume, Style::TEXT)
    };
    if let Some(tex) = icon_texture(ui.ctx(), glyph, GLANCE_ICON, tint) {
        let icon_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(GLANCE_ICON, GLANCE_ICON));
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    response.clicked()
}

/// The battery glanceable — `(percentage, tone)` for the system pack, or `None`
/// when no battery is present (a desktop / pre-poll). The tray's system-pack
/// pick + tone ladder, restated (lock 4).
fn battery_glance(seat: Option<&SeatSnapshot>) -> Option<(f64, Color32)> {
    let cells = seat?.batteries.present()?;
    let b = cells.iter().find(|b| b.power_supply).or_else(|| {
        cells.iter().max_by(|a, b| {
            a.percentage
                .partial_cmp(&b.percentage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    })?;
    Some((b.percentage, battery_glance_tone(b)))
}

/// The battery dot's tone (the tray's `battery_tone` ladder, restated): a
/// charging/full pack reads OK; a draining pack reads red at/under ~5% and amber
/// under ~20%; anything else the neutral dim dot.
fn battery_glance_tone(b: &Battery) -> Color32 {
    match b.state {
        BatteryState::Charging | BatteryState::FullyCharged => Style::OK,
        BatteryState::Empty => Style::DANGER,
        BatteryState::Discharging | BatteryState::PendingDischarge => {
            if b.percentage <= GLANCE_BATTERY_CRITICAL {
                Style::DANGER
            } else if b.percentage < GLANCE_BATTERY_LOW {
                Style::WARN
            } else {
                Style::TEXT_DIM
            }
        }
        BatteryState::PendingCharge | BatteryState::Unknown => Style::TEXT_DIM,
    }
}

/// The mesh-health glanceable `(tone, word)` — the worst-of lighthouse verdict,
/// dim/"—" before the first snapshot (lock 4).
const fn mesh_glance(mesh: &MeshSummary) -> (Color32, &'static str) {
    if !mesh.seen {
        return (Style::TEXT_DIM, "Mesh \u{2014}");
    }
    match mesh.health {
        LighthouseHealth::AllHealthy => (Style::OK, "Mesh OK"),
        LighthouseHealth::Degraded => (Style::DANGER, "Mesh degraded"),
        LighthouseHealth::None => (Style::TEXT_DIM, "Mesh \u{2014}"),
    }
}

// ──────────────────────────────────── tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::MeshSummary;
    use crate::dock::{self, Surface};
    use mde_seat::{Backend, BatteryKind, MixerStatus, MixerStrip, Probe, StripOrigin};
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
        assert!(
            matches!(c.phase, Phase::Verifying),
            "submit must enter Verifying"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(c.phase, Phase::Verifying) {
            c.tick(0.01);
            assert!(
                Instant::now() < deadline,
                "the verdict never landed off-thread"
            );
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

    // ── the CURTAIN-4 transport fakes + seat fixtures ──

    /// A recording [`LockMedia`]: scripts a now-playing view and counts the
    /// transport drives — the seam CURTAIN-4's live `MediaSurface` fills, and
    /// the recording fake §7 asks for (assert the locked strip drove the seam).
    #[derive(Default)]
    struct RecordingMedia {
        np: Option<NowPlaying>,
        toggles: usize,
        nexts: usize,
        prevs: usize,
    }

    impl RecordingMedia {
        /// A fake whose player is loaded + playing `title`.
        fn playing(title: &str) -> Self {
            Self {
                np: Some(NowPlaying {
                    title: title.to_owned(),
                    playing: true,
                }),
                ..Self::default()
            }
        }
    }

    impl LockMedia for RecordingMedia {
        fn now_playing(&self) -> Option<NowPlaying> {
            self.np.clone()
        }
        fn toggle_play(&mut self) {
            self.toggles += 1;
        }
        fn next(&mut self) {
            self.nexts += 1;
        }
        fn prev(&mut self) {
            self.prevs += 1;
        }
    }

    /// The recorded master-mixer writes (shared so the test reads them back).
    #[derive(Default)]
    struct MixerLog {
        volumes: Vec<(String, u8)>,
        mutes: Vec<(String, bool)>,
    }

    /// A recording [`LockMixer`]: logs every confirmed master write, or refuses
    /// them all to exercise the honest error path (§7).
    #[derive(Clone, Default)]
    struct RecordingMixer {
        log: std::rc::Rc<std::cell::RefCell<MixerLog>>,
        refuse: bool,
    }

    fn refused() -> SeatError {
        SeatError::Unavailable {
            backend: Backend::PipeWire,
            reason: "test refusal".to_owned(),
        }
    }

    impl LockMixer for RecordingMixer {
        fn set_volume(&self, id: &str, v: u8) -> Result<(), SeatError> {
            if self.refuse {
                return Err(refused());
            }
            self.log.borrow_mut().volumes.push((id.to_owned(), v));
            Ok(())
        }
        fn set_muted(&self, id: &str, m: bool) -> Result<(), SeatError> {
            if self.refuse {
                return Err(refused());
            }
            self.log.borrow_mut().mutes.push((id.to_owned(), m));
            Ok(())
        }
    }

    /// A typed-absent probe of any section (the honest build-host state).
    fn absent<T>() -> Probe<T> {
        Probe::Absent {
            backend: Backend::PipeWire,
            reason: "not available: test".to_owned(),
        }
    }

    /// An all-absent seat snapshot the per-section fixtures override.
    fn seat() -> SeatSnapshot {
        SeatSnapshot {
            bluetooth: absent(),
            batteries: absent(),
            on_ac: absent(),
            power: absent(),
            power_profile: absent(),
            charge_limit: absent(),
            lid: absent(),
            displays: absent(),
            backlights: absent(),
            mixer: absent(),
            ddc: absent(),
        }
    }

    /// A master mixer strip at a chosen volume + mute.
    fn mixer(volume: u8, muted: bool) -> MixerStatus {
        MixerStatus {
            master: MixerStrip {
                id: "master".to_owned(),
                name: "Master".to_owned(),
                origin: StripOrigin::HostSession,
                volume,
                muted,
            },
            strips: Vec::new(),
        }
    }

    /// One internal system pack at a chosen charge/state.
    fn pack(percentage: f64, state: BatteryState) -> Battery {
        Battery {
            model: "BAT0".to_owned(),
            kind: BatteryKind::Internal,
            percentage,
            state,
            power_supply: true,
            time_to_empty: None,
            time_to_full: None,
            energy_rate: None,
        }
    }

    /// A seen mesh summary at a chosen health.
    fn seen_mesh(health: LighthouseHealth) -> MeshSummary {
        MeshSummary {
            peers_total: 2,
            peers_online: 2,
            health,
            seen: true,
        }
    }

    /// Drive `show` with no live media / seat and a default mesh — the CURTAIN-1
    /// tests only exercise the clock / motion / input-exclusivity, not the
    /// CURTAIN-4 strip, so they mount an empty transport.
    fn show_bare(c: &mut Curtain, ctx: &egui::Context) {
        let mut media = RecordingMedia::default();
        c.show(ctx, &mut media, None, &MeshSummary::default());
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
        assert!(
            !c.stage_accepts_input(),
            "the field disables behind the wall"
        );
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

    fn click_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn raw_input_folds_to_wake_reveal_and_escape() {
        // A pointer drift only wakes (never a phantom reveal).
        let s = fold_events(&[egui::Event::PointerMoved(egui::pos2(1.0, 1.0))], None);
        assert_eq!(
            s,
            Signals {
                wake: true,
                reveal: false,
                escape: false
            }
        );
        // A key press wakes AND reveals; a release is nothing.
        let s = fold_events(&[key(egui::Key::A, true)], None);
        assert_eq!(
            s,
            Signals {
                wake: true,
                reveal: true,
                escape: false
            }
        );
        assert_eq!(
            fold_events(&[key(egui::Key::A, false)], None),
            Signals::default()
        );
        // A click reveals.
        let s = fold_events(&[click_at(egui::pos2(10.0, 10.0))], None);
        assert!(s.reveal && s.wake && !s.escape);
        // Escape carries the escape signal.
        let s = fold_events(&[key(egui::Key::Escape, true)], None);
        assert!(s.escape && s.wake);
        // A non-user event is nothing — it must not wake the dim.
        assert_eq!(
            fold_events(&[egui::Event::PointerGone], None),
            Signals::default()
        );
    }

    #[test]
    fn a_press_on_the_exempt_media_strip_wakes_but_never_reveals() {
        // CURTAIN-4 (design lock 3): a press inside the now-playing / volume
        // strip drives that control — it must NOT reveal the password stage —
        // while a press anywhere else on the face reveals it (lock 8).
        let strip = egui::Rect::from_min_max(egui::pos2(100.0, 300.0), egui::pos2(400.0, 380.0));
        let inside = fold_events(&[click_at(egui::pos2(250.0, 340.0))], Some(strip));
        assert_eq!(
            inside,
            Signals {
                wake: true,
                reveal: false,
                escape: false
            },
            "a press on the exempt media strip wakes but must not reveal"
        );
        let outside = fold_events(&[click_at(egui::pos2(250.0, 500.0))], Some(strip));
        assert!(
            outside.reveal && outside.wake,
            "a press off the strip still reveals the stage"
        );
        // A keystroke reveals regardless of the exempt strip (you can't type a
        // password into a transport button).
        let keyed = fold_events(&[key(egui::Key::A, true)], Some(strip));
        assert!(keyed.reveal);
    }

    // ── input exclusivity (lock 10) ──

    /// Mount a generic bottom **chrome strip** standing in for the shell's dock —
    /// one full-width clickable band that routes to the Workbench on a click — plus
    /// (optionally) the engaged curtain drawn after it (the render order in
    /// `main.rs`), for one headless frame. The real chrome is the left vertical dock
    /// (VDOCK); this input-exclusivity test only needs an interactive region under
    /// the curtain to prove the whole-screen grab (lock 10) swallows a click meant
    /// for the chrome beneath.
    fn shell_frame(
        ctx: &egui::Context,
        active: &mut Surface,
        mut curtain: Option<&mut Curtain>,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::TopBottomPanel::bottom("shell-chrome")
                .exact_height(dock::TASKBAR_H)
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| {
                    let (_rect, resp) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
                    if resp.clicked() {
                        *active = Surface::Workbench;
                    }
                });
            // Reborrow per frame — `ctx.run`'s closure is `FnMut`.
            if let Some(c) = curtain.as_deref_mut() {
                show_bare(c, ctx);
            }
        });
    }

    #[test]
    fn the_engaged_curtain_swallows_clicks_before_the_dock() {
        // A point inside the bottom chrome band (any x in the band works; the y is
        // the band's vertical centre). With no curtain the click routes to Workbench;
        // under the engaged curtain the whole-screen grab swallows it.
        let click = egui::pos2(32.0, 600.0 - dock::TASKBAR_H / 2.0);
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
        let _ = ctx.run(raw(Vec::new()), |ctx| show_bare(&mut c, ctx));
        assert_ne!(
            ctx.memory(egui::Memory::focused),
            Some(beneath),
            "keyboard focus must not stay under the curtain (lock 10)"
        );

        // …any key reveals the stage, whose field takes the keyboard.
        let _ = ctx.run(raw(vec![key(egui::Key::A, true)]), |ctx| {
            show_bare(&mut c, ctx);
        });
        assert!(matches!(c.phase, Phase::Revealing));
        let _ = ctx.run(raw(Vec::new()), |ctx| show_bare(&mut c, ctx));
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
        let _ = ctx.run(raw(Vec::new()), |ctx| show_bare(&mut c, ctx));
        let out = ctx.run(raw(Vec::new()), |ctx| show_bare(&mut c, ctx));
        let mut texts = 0;
        for clipped in &out.shapes {
            count_text_shapes(&clipped.shape, &mut texts);
        }
        // The face carries the clock's two lines (HH:MM + date) plus the
        // always-present CURTAIN-4 glanceables (nothing-playing · volume ·
        // mesh · date), so at least the clock's two must paint.
        assert!(
            texts >= 2,
            "the resting face is missing the clock's two lines ({texts} texts)"
        );
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the curtain painted no draw primitives");
    }

    #[test]
    fn an_unlocked_curtain_paints_and_claims_nothing() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = Curtain::default();
        let out = ctx.run(raw(Vec::new()), |ctx| show_bare(&mut c, ctx));
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            prims.is_empty(),
            "an unlocked curtain must draw nothing at all"
        );
    }

    // ── CURTAIN-4: the now-playing transport + volume + status glanceables ──

    #[test]
    fn the_media_seam_drives_the_real_media_surface() {
        // §7: the transport drives the LIVE player, not a mock — the production
        // `MediaSurface` (airgap-safe `FakeMpv`) through the `LockMedia` seam.
        let mut media = mde_media_egui::real_media();
        assert!(
            LockMedia::now_playing(&media).is_none(),
            "an idle surface honestly has nothing playing"
        );

        // Load + play a path through the real transport (no disk I/O — FakeMpv).
        media.dispatch(TransportAction::PlayPath("/media/song.mp3".to_owned()));
        let np = LockMedia::now_playing(&media).expect("a loaded track is now-playing");
        assert!(
            np.title.contains("song"),
            "the title is the real player's, not invented: {}",
            np.title
        );

        // The transport verbs reach the real player; a Stop unloads it, so the
        // strip honestly falls back to "nothing playing" (no dead controls).
        LockMedia::toggle_play(&mut media);
        media.dispatch(TransportAction::Stop);
        assert!(
            LockMedia::now_playing(&media).is_none(),
            "Stop unloads the transport — the honest empty state (§7)"
        );
    }

    #[test]
    fn the_media_strip_is_exempt_from_the_lock_but_the_rest_reveals() {
        // §7 / design lock 3: a press on the now-playing / volume strip drives
        // that control while the curtain stays LOCKED (the media hit-areas are
        // exempt from the whole-screen grab); a press anywhere else reveals the
        // password stage (lock 8).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = locked(Scripted::denies(0));
        let mut media = RecordingMedia::playing("Song \u{2014} Artist");
        let snap = {
            let mut s = seat();
            s.mixer = Probe::Present(mixer(40, false));
            s
        };
        let mesh = MeshSummary::default();

        // Prime a frame so the covering Area is sized + settled on the face.
        let _ = ctx.run(raw(Vec::new()), |ctx| {
            c.show(ctx, &mut media, Some(&snap), &mesh);
        });
        assert!(matches!(c.phase, Phase::Locked { .. }));

        // A press INSIDE the extras strip (its centre, 0.52·h) must NOT reveal.
        let inside = egui::pos2(640.0, 720.0 * EXTRAS_Y_FRAC);
        let _ = ctx.run(
            raw(vec![egui::Event::PointerMoved(inside), click_at(inside)]),
            |ctx| {
                c.show(ctx, &mut media, Some(&snap), &mesh);
            },
        );
        assert!(
            matches!(c.phase, Phase::Locked { .. }),
            "a press on the media strip must keep the curtain locked (lock 3)"
        );

        // A press near the top of the face (off the strip) reveals the stage.
        let outside = egui::pos2(640.0, 40.0);
        let _ = ctx.run(
            raw(vec![egui::Event::PointerMoved(outside), click_at(outside)]),
            |ctx| {
                c.show(ctx, &mut media, Some(&snap), &mesh);
            },
        );
        assert!(
            matches!(c.phase, Phase::Revealing),
            "a press off the strip reveals the password stage (lock 8)"
        );
    }

    #[test]
    fn a_player_less_strip_still_reveals_on_a_press_over_it() {
        // With no player AND no mixer the strip carries no live control, so its
        // area is NOT exempt — a press there must still reveal (never a dead
        // zone that traps the user, §7).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = locked(Scripted::denies(0));
        let mut media = RecordingMedia::default(); // nothing playing
        let mesh = MeshSummary::default();
        let _ = ctx.run(raw(Vec::new()), |ctx| {
            c.show(ctx, &mut media, None, &mesh);
        });
        let over = egui::pos2(640.0, 720.0 * EXTRAS_Y_FRAC);
        let _ = ctx.run(
            raw(vec![egui::Event::PointerMoved(over), click_at(over)]),
            |ctx| {
                c.show(ctx, &mut media, None, &mesh);
            },
        );
        assert!(
            matches!(c.phase, Phase::Revealing),
            "an inert strip must not swallow the reveal"
        );
    }

    #[test]
    fn the_playing_face_paints_more_than_the_idle_face() {
        // The transport strip actually renders: a playing face (title + state +
        // prev/play/next) carries strictly more text than an idle one.
        fn face_texts(np: Option<&str>) -> usize {
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let mut c = locked(Scripted::denies(0));
            let mut media = np.map_or_else(RecordingMedia::default, RecordingMedia::playing);
            let mesh = MeshSummary::default();
            let _ = ctx.run(raw(Vec::new()), |ctx| {
                c.show(ctx, &mut media, None, &mesh);
            });
            let out = ctx.run(raw(Vec::new()), |ctx| {
                c.show(ctx, &mut media, None, &mesh);
            });
            let mut texts = 0;
            for clipped in &out.shapes {
                count_text_shapes(&clipped.shape, &mut texts);
            }
            texts
        }
        assert!(
            face_texts(Some("Great Song")) > face_texts(None),
            "the now-playing strip must add its title + transport to the face"
        );
    }

    #[test]
    fn the_volume_slider_drives_the_real_mixer_and_echoes_it_back() {
        // §7 / design lock 3: the slider drives the REAL host mixer through the
        // seam; the confirmed write echoes so the level reads back before the
        // ~5s seat poll (the tray's echo, one slot).
        let mut c = Curtain::with_verifier(Scripted::denies(0));
        let rec = RecordingMixer::default();
        c.mixer = Box::new(rec.clone());
        let now = Instant::now();

        c.drive_volume("master", 55, false, now);
        assert_eq!(
            rec.log.borrow().volumes,
            vec![("master".to_owned(), 55)],
            "the confirmed write reached the mixer seam"
        );
        // The echo folds over a stale snapshot so the slider reads 55 at once.
        let snap = {
            let mut s = seat();
            s.mixer = Probe::Present(mixer(40, false)); // the poll still says 40
            s
        };
        let view = c.master_view(Some(&snap), now).expect("a master strip");
        assert_eq!(
            view.volume, 55,
            "the echo reads the just-written level back"
        );

        // Once the snapshot catches up (or the TTL passes), the echo clears.
        let caught = {
            let mut s = seat();
            s.mixer = Probe::Present(mixer(55, false));
            s
        };
        c.reconcile_vol_echo(Some(&caught), now);
        assert!(
            c.vol_echo.is_none(),
            "the echo drops once the poll catches up"
        );
    }

    #[test]
    fn the_mute_toggle_drives_the_mixer_and_a_refusal_surfaces_honestly() {
        // A confirmed mute echoes; a refused write surfaces its typed error and
        // echoes NOTHING (§7 — the stale snapshot stays the truth).
        let mut c = Curtain::with_verifier(Scripted::denies(0));
        let rec = RecordingMixer::default();
        c.mixer = Box::new(rec.clone());
        let now = Instant::now();
        c.drive_mute("master", true, 40, now);
        assert_eq!(rec.log.borrow().mutes, vec![("master".to_owned(), true)]);
        assert!(c.vol_echo.is_some(), "a confirmed mute echoes");

        let mut refuse = Curtain::with_verifier(Scripted::denies(0));
        refuse.mixer = Box::new(RecordingMixer {
            refuse: true,
            ..RecordingMixer::default()
        });
        refuse.drive_volume("master", 10, false, now);
        assert!(
            refuse.vol_echo.is_none(),
            "a refused write never echoes (§7)"
        );
        assert!(
            refuse
                .mixer_error
                .as_deref()
                .unwrap_or_default()
                .contains("volume"),
            "a refused write surfaces its typed error"
        );
    }

    #[test]
    fn the_status_folds_battery_mesh_and_show_no_chat() {
        // Design lock 4: the glanceables read real battery + mesh; the curtain
        // is handed NO chat state (the `show` signature takes only media / seat /
        // mesh), so message content can never reach the face.
        let mut s = seat();
        s.batteries = Probe::Present(vec![pack(12.0, BatteryState::Discharging)]);
        let (pct, tone) = battery_glance(Some(&s)).expect("a system pack");
        assert!((pct - 12.0).abs() < f64::EPSILON);
        assert_eq!(tone, Style::WARN, "12% draining reads amber");
        assert!(
            battery_glance(None).is_none(),
            "no seat → no battery glanceable"
        );

        assert_eq!(
            mesh_glance(&seen_mesh(LighthouseHealth::AllHealthy)),
            (Style::OK, "Mesh OK")
        );
        assert_eq!(
            mesh_glance(&seen_mesh(LighthouseHealth::Degraded)).0,
            Style::DANGER
        );
        assert_eq!(
            mesh_glance(&MeshSummary::default()).0,
            Style::TEXT_DIM,
            "a pre-first-snapshot mesh reads dim, not a fabricated verdict"
        );
    }

    #[test]
    fn an_absent_mixer_shows_no_dead_slider() {
        // §7: with no mixer the master view is None, so the volume row renders
        // the honest not-available line instead of a slider over nothing.
        let c = Curtain::with_verifier(Scripted::denies(0));
        assert!(
            c.master_view(Some(&seat()), Instant::now()).is_none(),
            "an absent mixer yields no master view (honest, no dead slider)"
        );
        assert!(c.master_view(None, Instant::now()).is_none());
    }
}
