//! POWER-5 — the DRM-native idle + lid honorer.
//!
//! A compositor-less DRM shell has no swayidle / Wayland idle-notify and no
//! logind lid-switch handler acting for it — so the shell itself has to honor an
//! idle timeout and a lid close. This module is that honorer: a tiny per-frame
//! [`PowerHonor::tick`] folded into the shell's update loop, plus the persisted
//! [`PowerHonorConfig`] the System surface's Power section edits.
//!
//! Two mechanisms, both driving the ONE [`mde_seat::Seat`] (lock 1) through the
//! System state's [`crate::system::SystemState::honor_power`]:
//!
//! - **Idle timer** — the last user-input instant is tracked from egui's per-frame
//!   input (any key/text/pointer/scroll/touch/zoom resets it). When the seat has
//!   been idle at least the configured timeout, the idle action fires **once**
//!   ([`PowerVerb::Suspend`]) and does not re-fire until activity resumes. The
//!   timeout defaults to **Never** — a fresh install never surprise-suspends; only
//!   an operator-set timeout arms it (the safe default).
//! - **Lid handler** — the [`mde_seat::SeatSnapshot::lid`] reading each tick; on an
//!   Open→Closed edge the configured [`LidAction`] fires once (Suspend default /
//!   Lock / Do nothing). A held-closed lid, a repeated Closed read, or an Unknown
//!   flap never re-fires (the debounce is the edge, not the level), and a lid that
//!   is already closed at startup never fires (it was never seen open, so unarmed).
//!
//! Everything here is decoupled from egui + the real seat behind a pure state fold
//! ([`PowerHonor::step`]) and pure decisions ([`idle_should_fire`] / [`lid_step`]),
//! so the idle-elapsed rule, the lid transition→action mapping, and the config
//! round-trip are all unit-tested without ever calling suspend (§7 runtime-real,
//! the real suspend/lid is HW-gated).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mde_egui::egui;
use mde_seat::{LidState, PowerVerb};

use crate::system::SystemState;

/// The honorer config filename under the shell's client data dir.
const CONFIG_FILE: &str = "power-honor.json";

/// While an idle timeout is armed but unfired, keep a wake scheduled this often so
/// the timer fires promptly even with no other input (it also piggybacks on the
/// System poll's heartbeat, so this is only a floor). Minute-granular timeouts do
/// not need finer resolution.
const IDLE_CHECK: Duration = Duration::from_secs(5);

// ──────────────────────────── config types ────────────────────────────

/// What closing the laptop lid does. The safe laptop default is Suspend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LidAction {
    /// Suspend the host (suspend-to-RAM) — the default.
    #[default]
    Suspend,
    /// Lock the session.
    Lock,
    /// Do nothing.
    Nothing,
}

impl LidAction {
    /// Every action, in picker order.
    pub(crate) const ALL: [Self; 3] = [Self::Suspend, Self::Lock, Self::Nothing];

    /// The operator-facing label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Suspend => "Suspend",
            Self::Lock => "Lock",
            Self::Nothing => "Do nothing",
        }
    }

    /// The power verb this action performs, or `None` for [`LidAction::Nothing`].
    const fn verb(self) -> Option<PowerVerb> {
        match self {
            Self::Suspend => Some(PowerVerb::Suspend),
            Self::Lock => Some(PowerVerb::Lock),
            Self::Nothing => None,
        }
    }
}

/// The persisted honorer settings the Power section edits and the honorer enforces.
///
/// The [`Default`] is the SAFE default: `idle_timeout_min: None` (idle-suspend off,
/// so a fresh install never surprise-suspends until the operator arms it) and
/// `lid_action: LidAction::Suspend` (the laptop-expected close behavior). Both fall
/// out of the field defaults ([`Option::default`] = `None`, [`LidAction::default`] =
/// `Suspend`), so the derive is exactly this policy.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct PowerHonorConfig {
    /// Idle-suspend timeout in whole minutes; `None` = Never (off) — the safe default.
    #[serde(default)]
    pub(crate) idle_timeout_min: Option<u64>,
    /// What closing the lid does (default Suspend).
    #[serde(default)]
    pub(crate) lid_action: LidAction,
}

impl PowerHonorConfig {
    /// The idle timeout as a [`Duration`], or `None` when set to Never.
    pub(crate) fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout_min
            .map(|m| Duration::from_secs(m.saturating_mul(60)))
    }

    /// The default config path (`<client-data-dir>/power-honor.json`), or `None`
    /// when no data dir resolves (a headless context with no bus + no home).
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(CONFIG_FILE))
    }

    /// Load from `path`, honestly folding a missing / half-written / malformed file
    /// to the safe defaults (never a fatal, never a fabricated setting).
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Load from the default path (safe defaults when absent / unresolvable).
    #[must_use]
    pub(crate) fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` (atomic temp + rename, like the mesh peer/prefs records).
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persist to the default path (a silent no-op when no data dir resolves).
    pub(crate) fn save(&self) {
        if let Some(path) = Self::default_path() {
            let _ = self.save_to(&path);
        }
    }
}

// ──────────────────────────── pure decisions (unit-tested) ────────────────────────────

/// Whether the idle action should fire: only when a timeout is armed AND the seat
/// has been idle at least that long. `None` (Never) never fires — the safe default.
#[must_use]
fn idle_should_fire(idle_for: Duration, timeout: Option<Duration>) -> bool {
    timeout.is_some_and(|t| idle_for >= t)
}

/// One lid step: given whether the lid is currently "armed" (has been seen open)
/// and this tick's reading, return the new armed flag and whether to fire the lid
/// action. Firing is exactly the Open→Closed edge — a held-closed lid, a repeated
/// Closed read, or an Unknown flap never re-fires (the debounce), and a lid already
/// closed at startup is unarmed so it never surprise-acts.
#[must_use]
const fn lid_step(armed: bool, reading: LidState) -> (bool, bool) {
    match reading {
        // Open arms the next close (and clears a prior fire).
        LidState::Open => (true, false),
        // Closed fires only if we were armed, then disarms (no re-fire while held).
        LidState::Closed => (false, armed),
        // Unknown is ignored — hold the armed state, never act on a guess.
        LidState::Unknown => (armed, false),
    }
}

// ──────────────────────────── the honorer ────────────────────────────

/// The idle + lid honorer — the per-frame runtime state (the config is the source
/// of truth on [`SystemState`], read each tick). Held by the shell, ticked once per
/// frame; drives Suspend/Lock through the ONE seat.
pub(crate) struct PowerHonor {
    /// The instant of the last observed user activity (idle is measured from here).
    last_activity: Instant,
    /// Whether the idle action has already fired for the current idle stretch —
    /// reset the moment activity resumes, so it fires at most once per idle period.
    idle_fired: bool,
    /// Whether the lid is currently "armed" (seen open) so the next close acts once.
    lid_armed: bool,
}

impl Default for PowerHonor {
    fn default() -> Self {
        Self {
            last_activity: Instant::now(),
            idle_fired: false,
            lid_armed: false,
        }
    }
}

impl PowerHonor {
    /// The per-frame hook (the shell's one-line update-loop call): fold this frame's
    /// egui input + the seat's lid reading into the idle/lid decision, and drive any
    /// resulting power verb through the ONE seat. A typed failure is kept as an
    /// honest note, never a panic.
    pub(crate) fn tick(&mut self, ctx: &egui::Context, system: &SystemState) {
        let active = ctx_has_activity(ctx);
        let lid = system.lid_state();
        let cfg = system.power_honor_config();
        let verbs = self.step(cfg.idle_timeout(), cfg.lid_action, active, lid, Instant::now());
        for verb in verbs {
            // A refused/absent logind is surfaced honestly to the journal, never a
            // pretend-success and never a panic (§7). It fires at most once per idle
            // stretch / lid edge, so this can't spam the log.
            if let Err(e) = system.honor_power(verb) {
                eprintln!("power-honor: {} failed: {e}", verb.label());
            }
        }
        // Keep a wake scheduled while an idle timeout is armed but unfired, so the
        // timer fires promptly even with no other input.
        if cfg.idle_timeout().is_some() && !self.idle_fired {
            ctx.request_repaint_after(IDLE_CHECK);
        }
    }

    /// The pure state fold (unit-tested without egui or a real seat): update the idle
    /// timer + lid arm from this tick's input, returning the verbs to execute (at
    /// most one idle + one lid). No I/O, no suspend — the caller executes.
    fn step(
        &mut self,
        idle_timeout: Option<Duration>,
        lid_action: LidAction,
        active: bool,
        lid: Option<LidState>,
        now: Instant,
    ) -> Vec<PowerVerb> {
        let mut verbs = Vec::new();
        // ── idle timer ──
        if active {
            self.last_activity = now;
            self.idle_fired = false;
        }
        if !self.idle_fired {
            let idle_for = now.saturating_duration_since(self.last_activity);
            if idle_should_fire(idle_for, idle_timeout) {
                self.idle_fired = true;
                verbs.push(PowerVerb::Suspend);
            }
        }
        // ── lid handler ──
        if let Some(reading) = lid {
            let (armed, fire) = lid_step(self.lid_armed, reading);
            self.lid_armed = armed;
            if fire {
                if let Some(verb) = lid_action.verb() {
                    verbs.push(verb);
                }
            }
        }
        verbs
    }
}

/// Whether this frame's egui input carries genuine user activity (key/text/pointer/
/// scroll/touch/zoom) — the idle timer resets on any of these. Non-user events (a
/// window-focus flip, a screenshot reply, the pointer leaving) are deliberately not
/// activity, so they never keep the seat awake.
fn ctx_has_activity(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.events.iter().any(is_user_activity))
}

/// Classify one egui event as genuine user activity (see [`ctx_has_activity`]).
const fn is_user_activity(e: &egui::Event) -> bool {
    use egui::Event;
    matches!(
        e,
        Event::Key { .. }
            | Event::Text(_)
            | Event::Paste(_)
            | Event::Copy
            | Event::Cut
            | Event::PointerMoved(_)
            | Event::MouseMoved(_)
            | Event::PointerButton { .. }
            | Event::MouseWheel { .. }
            | Event::Zoom(_)
            | Event::Touch { .. }
            | Event::Ime(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the config round-trip + safe defaults ──

    #[test]
    fn the_safe_default_is_never_suspend_and_lid_suspend() {
        let d = PowerHonorConfig::default();
        // A fresh install never surprise-suspends: idle is Never (off).
        assert_eq!(d.idle_timeout_min, None);
        assert_eq!(d.idle_timeout(), None);
        // The laptop-expected lid default is Suspend.
        assert_eq!(d.lid_action, LidAction::Suspend);
    }

    #[test]
    fn idle_timeout_minutes_fold_to_a_duration() {
        assert_eq!(
            PowerHonorConfig {
                idle_timeout_min: Some(5),
                ..PowerHonorConfig::default()
            }
            .idle_timeout(),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            PowerHonorConfig {
                idle_timeout_min: Some(30),
                ..PowerHonorConfig::default()
            }
            .idle_timeout(),
            Some(Duration::from_secs(1800))
        );
    }

    fn temp_root(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mde-power5-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn config_round_trips_through_disk_and_folds_a_missing_file_to_default() {
        let dir = temp_root("cfg");
        std::fs::create_dir_all(&dir).expect("mkroot");
        let path = dir.join(CONFIG_FILE);

        let cfg = PowerHonorConfig {
            idle_timeout_min: Some(10),
            lid_action: LidAction::Lock,
        };
        cfg.save_to(&path).expect("save");
        assert_eq!(PowerHonorConfig::load_from(&path), cfg, "round-trip");

        // A missing file folds to the safe defaults, never a fatal.
        assert_eq!(
            PowerHonorConfig::load_from(&dir.join("nope.json")),
            PowerHonorConfig::default()
        );
        // A malformed file folds to defaults too (never a panic / fabricated value).
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ not json").expect("write bad");
        assert_eq!(
            PowerHonorConfig::load_from(&bad),
            PowerHonorConfig::default()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── the pure idle decision ──

    #[test]
    fn idle_never_fires_when_the_timeout_is_off() {
        // Never (None) is the safe default — no elapsed idle ever fires it.
        assert!(!idle_should_fire(Duration::from_secs(86_400), None));
    }

    #[test]
    fn idle_fires_only_at_or_past_the_timeout() {
        let t = Some(Duration::from_secs(300));
        assert!(!idle_should_fire(Duration::from_secs(299), t));
        assert!(idle_should_fire(Duration::from_secs(300), t));
        assert!(idle_should_fire(Duration::from_secs(301), t));
    }

    // ── the pure lid transition ──

    #[test]
    fn lid_fires_only_on_an_open_to_closed_edge() {
        // A lid already closed at startup (unarmed) never fires.
        assert_eq!(lid_step(false, LidState::Closed), (false, false));
        // Open arms it.
        assert_eq!(lid_step(false, LidState::Open), (true, false));
        // Armed + Closed → fire once, then disarm.
        assert_eq!(lid_step(true, LidState::Closed), (false, true));
        // Held closed (already disarmed) → no re-fire.
        assert_eq!(lid_step(false, LidState::Closed), (false, false));
        // Unknown holds the armed state and never fires (no guessing).
        assert_eq!(lid_step(true, LidState::Unknown), (true, false));
        assert_eq!(lid_step(false, LidState::Unknown), (false, false));
    }

    // ── the integrated fold (timer + lid state machine) ──

    #[test]
    fn step_fires_idle_once_and_not_again_until_activity() {
        let mut h = PowerHonor::default();
        let t = Some(Duration::from_secs(300));
        let base = Instant::now();

        // An active tick seeds last-activity and never fires.
        assert!(h
            .step(t, LidAction::Nothing, true, None, base)
            .is_empty());
        // Before the timeout: nothing.
        assert!(h
            .step(t, LidAction::Nothing, false, None, base + Duration::from_secs(299))
            .is_empty());
        // At the timeout: Suspend fires exactly once.
        assert_eq!(
            h.step(t, LidAction::Nothing, false, None, base + Duration::from_secs(300)),
            vec![PowerVerb::Suspend]
        );
        // Still idle, well past: no re-fire.
        assert!(h
            .step(t, LidAction::Nothing, false, None, base + Duration::from_secs(900))
            .is_empty());

        // Activity resumes → the latch clears; a full fresh timeout must elapse.
        let base2 = base + Duration::from_secs(1_000);
        assert!(h
            .step(t, LidAction::Nothing, true, None, base2)
            .is_empty());
        assert!(h
            .step(t, LidAction::Nothing, false, None, base2 + Duration::from_secs(299))
            .is_empty());
        assert_eq!(
            h.step(t, LidAction::Nothing, false, None, base2 + Duration::from_secs(300)),
            vec![PowerVerb::Suspend],
            "it arms again after activity"
        );
    }

    #[test]
    fn step_never_suspends_on_idle_when_the_timeout_is_never() {
        let mut h = PowerHonor::default();
        let base = Instant::now();
        h.step(None, LidAction::Nothing, true, None, base);
        // A day of idle with Never set: still nothing (the safe default).
        assert!(h
            .step(None, LidAction::Nothing, false, None, base + Duration::from_secs(86_400))
            .is_empty());
    }

    #[test]
    fn step_maps_the_lid_close_to_the_configured_action_and_debounces() {
        let mut h = PowerHonor::default();
        let now = Instant::now();

        // Closed at startup (never seen open) → no surprise action.
        assert!(h
            .step(None, LidAction::Suspend, false, Some(LidState::Closed), now)
            .is_empty());
        // Open, then Closed → the configured Suspend fires once.
        assert!(h
            .step(None, LidAction::Suspend, false, Some(LidState::Open), now)
            .is_empty());
        assert_eq!(
            h.step(None, LidAction::Suspend, false, Some(LidState::Closed), now),
            vec![PowerVerb::Suspend]
        );
        // Held closed → no loop.
        assert!(h
            .step(None, LidAction::Suspend, false, Some(LidState::Closed), now)
            .is_empty());
        // An Unknown flap → no fire.
        assert!(h
            .step(None, LidAction::Suspend, false, Some(LidState::Unknown), now)
            .is_empty());

        // Reopen + reclose with Lock configured → the Lock verb fires.
        h.step(None, LidAction::Lock, false, Some(LidState::Open), now);
        assert_eq!(
            h.step(None, LidAction::Lock, false, Some(LidState::Closed), now),
            vec![PowerVerb::Lock]
        );

        // With Nothing configured, a close performs no verb at all.
        h.step(None, LidAction::Nothing, false, Some(LidState::Open), now);
        assert!(h
            .step(None, LidAction::Nothing, false, Some(LidState::Closed), now)
            .is_empty());
    }

    #[test]
    fn a_desktop_with_no_lid_reading_never_fires() {
        // `lid: None` is the Absent probe (a desktop) — the honorer never acts.
        let mut h = PowerHonor::default();
        let now = Instant::now();
        for _ in 0..5 {
            assert!(h.step(None, LidAction::Suspend, false, None, now).is_empty());
        }
    }

    #[test]
    fn lid_action_verbs_map_and_the_labels_read() {
        assert_eq!(LidAction::Suspend.verb(), Some(PowerVerb::Suspend));
        assert_eq!(LidAction::Lock.verb(), Some(PowerVerb::Lock));
        assert_eq!(LidAction::Nothing.verb(), None);
        assert_eq!(LidAction::Suspend.label(), "Suspend");
        assert_eq!(LidAction::Nothing.label(), "Do nothing");
        assert_eq!(LidAction::ALL.len(), 3);
    }
}
