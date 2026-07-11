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
//!   been idle at least the configured timeout, the configured [`IdleAction`] fires
//!   **once** (Suspend default / **Lock** / Do nothing) and does not re-fire until
//!   activity resumes. The timeout defaults to **Never** — a fresh install never
//!   surprise-suspends; only an operator-set timeout arms it (the safe default).
//! - **Lid handler** — the [`mde_seat::SeatSnapshot::lid`] reading each tick; on an
//!   Open→Closed edge the configured [`LidAction`] fires once (Suspend default /
//!   Lock / Do nothing). A held-closed lid, a repeated Closed read, or an Unknown
//!   flap never re-fires (the debounce is the edge, not the level), and a lid that
//!   is already closed at startup never fires (it was never seen open, so unarmed).
//!
//! **CURTAIN-3** folds the lock curtain into the honorer: an idle/lid action of
//! **Lock** does NOT route to logind — the DM-less shell IS this seat's locker, so
//! [`PowerHonor::tick`] reports the request and `main.rs` drops the in-process
//! [`crate::curtain::Curtain`] (exactly as Super+L does). The same config carries the
//! persisted boot-gate ([`PowerHonorConfig::require_login_at_boot`], default **on**):
//! [`should_lock_at_boot`] is the pure decision the shell reads once at construction
//! to start Locked before any surface renders.
//!
//! Everything here is decoupled from egui + the real seat behind a pure state fold
//! ([`PowerHonor::step`]) and pure decisions ([`idle_should_fire`] / [`lid_step`] /
//! [`should_lock_at_boot`]), so the idle-elapsed rule, the lid/idle action→verb
//! mapping, the boot-gate decision, and the config round-trip are all unit-tested
//! without ever calling suspend or a real curtain (§7 runtime-real, the real
//! suspend/lid is HW-gated).

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private shell module are this crate's idiom \
              (curtain, dock, tray, …); main.rs + the System surface's Power \
              section consume the honorer's config + action types"
)]

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

/// What firing the **idle timeout** does (CURTAIN-3). The prior POWER-5 behavior was
/// a hard-wired Suspend; that stays the default, and **Lock** (drop the curtain) joins
/// it beside Do nothing. Mirrors [`LidAction`] — the same {Suspend, Lock, Nothing} set
/// mapping to the same [`PowerVerb`]s — but kept a distinct type so the idle and lid
/// choices persist and read independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdleAction {
    /// Suspend the host (suspend-to-RAM) — the default, the prior POWER-5 behavior.
    #[default]
    Suspend,
    /// Lock the session — drop the in-process curtain (CURTAIN-3).
    Lock,
    /// Do nothing (the timeout arms nothing but still latches once).
    Nothing,
}

impl IdleAction {
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

    /// The power verb this action performs, or `None` for [`IdleAction::Nothing`].
    const fn verb(self) -> Option<PowerVerb> {
        match self {
            Self::Suspend => Some(PowerVerb::Suspend),
            Self::Lock => Some(PowerVerb::Lock),
            Self::Nothing => None,
        }
    }
}

/// The default for [`PowerHonorConfig::require_login_at_boot`]: **on** — the shipped
/// secure posture (the shell boots to the curtain). A serde default so a config file
/// written before CURTAIN-3 (no field) still reads as require-login, never silently
/// off.
const fn require_login_default() -> bool {
    true
}

/// The persisted honorer settings the Power section edits and the honorer enforces.
///
/// The [`Default`] is the SAFE / shipped default: `idle_timeout_min: None` (idle
/// action off, so a fresh install never surprise-suspends until the operator arms a
/// timeout), `idle_action: IdleAction::Suspend` and `lid_action: LidAction::Suspend`
/// (the laptop-expected behaviors), and `require_login_at_boot: true` (CURTAIN-3 — the
/// shell boots to the curtain). Only the boot-gate departs from a bare field-derive
/// (a `bool` derives `false`), so [`Default`] is written by hand to make the on-by-
/// default policy explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PowerHonorConfig {
    /// Idle timeout in whole minutes; `None` = Never (off) — the safe default.
    #[serde(default)]
    pub(crate) idle_timeout_min: Option<u64>,
    /// What firing the idle timeout does (default Suspend; CURTAIN-3 adds Lock).
    #[serde(default)]
    pub(crate) idle_action: IdleAction,
    /// What closing the lid does (default Suspend).
    #[serde(default)]
    pub(crate) lid_action: LidAction,
    /// CURTAIN-3 boot-gate: start the shell Locked (the curtain drops before any
    /// surface renders) — default **on**, the shipped secure posture.
    #[serde(default = "require_login_default")]
    pub(crate) require_login_at_boot: bool,
}

impl Default for PowerHonorConfig {
    fn default() -> Self {
        Self {
            idle_timeout_min: None,
            idle_action: IdleAction::Suspend,
            lid_action: LidAction::Suspend,
            require_login_at_boot: require_login_default(),
        }
    }
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

/// The CURTAIN-3 boot-gate decision: the shell starts **Locked** (the curtain drops
/// before any surface renders) exactly when the persisted policy requires a login at
/// boot. Pure, so the boot-lock rule is unit-tested without standing up the whole
/// shell; `main.rs` reads it once at construction.
#[must_use]
pub(crate) const fn should_lock_at_boot(cfg: &PowerHonorConfig) -> bool {
    cfg.require_login_at_boot
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
    /// resulting power verb through the ONE seat. Returns `true` when an idle/lid
    /// action of **Lock** fired this frame — CURTAIN-3 routes that in-process (the
    /// caller drops the shell's curtain), NOT to logind, since the DM-less shell IS
    /// this seat's locker (exactly like Super+L). A typed failure on the host verbs
    /// (Suspend) is kept as an honest note, never a panic.
    #[must_use]
    pub(crate) fn tick(&mut self, ctx: &egui::Context, system: &SystemState) -> bool {
        let active = ctx_has_activity(ctx);
        let lid = system.lid_state();
        let cfg = system.power_honor_config();
        let verbs = self.step(
            cfg.idle_timeout(),
            cfg.idle_action,
            cfg.lid_action,
            active,
            lid,
            Instant::now(),
        );
        let mut lock_requested = false;
        for verb in verbs {
            if matches!(verb, PowerVerb::Lock) {
                // CURTAIN-3: Lock is the shell's own curtain, dropped in-process by
                // the caller — never sent to logind's session Lock (that leg stays
                // for the System surface's explicit control). At most one per idle
                // stretch / lid edge, so a held lock never spams.
                lock_requested = true;
            } else if let Err(e) = system.honor_power(verb) {
                // A refused/absent logind is surfaced honestly to the journal, never
                // a pretend-success and never a panic (§7). test-obs-3: structured
                // now, so the auto idle/lid power path is filterable off-seat.
                tracing::error!(
                    target: "shell::power",
                    verb = verb.label(),
                    source = "idle-lid",
                    error = %e,
                    "automatic power verb failed",
                );
            }
        }
        // Keep a wake scheduled while an idle timeout is armed but unfired, so the
        // timer fires promptly even with no other input.
        if cfg.idle_timeout().is_some() && !self.idle_fired {
            ctx.request_repaint_after(IDLE_CHECK);
        }
        lock_requested
    }

    /// The pure state fold (unit-tested without egui or a real seat): update the idle
    /// timer + lid arm from this tick's input, returning the verbs to execute (at
    /// most one idle + one lid). No I/O, no suspend, no curtain — the caller executes
    /// (routing a [`PowerVerb::Lock`] to the in-process curtain, the rest to the seat).
    fn step(
        &mut self,
        idle_timeout: Option<Duration>,
        idle_action: IdleAction,
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
                // Latch once per idle stretch regardless of the action (Nothing still
                // arms only once); a Suspend/Lock verb rides out to the caller.
                self.idle_fired = true;
                if let Some(verb) = idle_action.verb() {
                    verbs.push(verb);
                }
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
        // The idle action defaults to the prior POWER-5 behavior (Suspend).
        assert_eq!(d.idle_action, IdleAction::Suspend);
        // The laptop-expected lid default is Suspend.
        assert_eq!(d.lid_action, LidAction::Suspend);
        // CURTAIN-3: the shipped default boots to the curtain (require login on).
        assert!(d.require_login_at_boot);
        assert!(should_lock_at_boot(&d));
    }

    #[test]
    fn the_boot_gate_decision_tracks_require_login_at_boot() {
        // On (the shipped default) → the shell starts Locked before any surface.
        assert!(should_lock_at_boot(&PowerHonorConfig::default()));
        // Off → the shell boots straight to the desktop (the old DM-less behavior).
        let off = PowerHonorConfig {
            require_login_at_boot: false,
            ..PowerHonorConfig::default()
        };
        assert!(!should_lock_at_boot(&off));
    }

    #[test]
    fn a_config_written_before_curtain3_still_reads_as_require_login() {
        // A power-honor.json from before the boot-gate field folds to require-login
        // (the serde default), never silently off — the secure posture is the floor.
        let legacy = r#"{ "idle_timeout_min": 5, "lid_action": "suspend" }"#;
        let cfg: PowerHonorConfig = serde_json::from_str(legacy).expect("legacy config");
        assert!(
            cfg.require_login_at_boot,
            "a pre-CURTAIN-3 file must default on"
        );
        assert_eq!(
            cfg.idle_action,
            IdleAction::Suspend,
            "and idle_action folds to Suspend"
        );
        assert_eq!(cfg.idle_timeout_min, Some(5));
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
            idle_action: IdleAction::Lock,
            lid_action: LidAction::Lock,
            require_login_at_boot: false,
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
            .step(t, IdleAction::Suspend, LidAction::Nothing, true, None, base)
            .is_empty());
        // Before the timeout: nothing.
        assert!(h
            .step(
                t,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                None,
                base + Duration::from_secs(299)
            )
            .is_empty());
        // At the timeout: Suspend fires exactly once.
        assert_eq!(
            h.step(
                t,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                None,
                base + Duration::from_secs(300)
            ),
            vec![PowerVerb::Suspend]
        );
        // Still idle, well past: no re-fire.
        assert!(h
            .step(
                t,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                None,
                base + Duration::from_secs(900)
            )
            .is_empty());

        // Activity resumes → the latch clears; a full fresh timeout must elapse.
        let base2 = base + Duration::from_secs(1_000);
        assert!(h
            .step(
                t,
                IdleAction::Suspend,
                LidAction::Nothing,
                true,
                None,
                base2
            )
            .is_empty());
        assert!(h
            .step(
                t,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                None,
                base2 + Duration::from_secs(299)
            )
            .is_empty());
        assert_eq!(
            h.step(
                t,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                None,
                base2 + Duration::from_secs(300)
            ),
            vec![PowerVerb::Suspend],
            "it arms again after activity"
        );
    }

    #[test]
    fn step_fires_the_configured_idle_action_lock_and_nothing() {
        // CURTAIN-3: idle_action = Lock → a Lock verb rides out (the caller drops the
        // in-process curtain); it never routes to logind here.
        let t = Some(Duration::from_secs(300));
        let base = Instant::now();
        let mut lock = PowerHonor::default();
        assert!(lock
            .step(t, IdleAction::Lock, LidAction::Nothing, true, None, base)
            .is_empty());
        assert_eq!(
            lock.step(
                t,
                IdleAction::Lock,
                LidAction::Nothing,
                false,
                None,
                base + Duration::from_secs(300)
            ),
            vec![PowerVerb::Lock],
            "idle-action=Lock must fire a Lock verb at the timeout"
        );

        // idle_action = Nothing → the timeout latches but performs no verb at all.
        let mut nothing = PowerHonor::default();
        assert!(nothing
            .step(t, IdleAction::Nothing, LidAction::Nothing, true, None, base)
            .is_empty());
        assert!(
            nothing
                .step(
                    t,
                    IdleAction::Nothing,
                    LidAction::Nothing,
                    false,
                    None,
                    base + Duration::from_secs(600)
                )
                .is_empty(),
            "idle-action=Nothing fires no verb even past the timeout"
        );
    }

    #[test]
    fn step_never_suspends_on_idle_when_the_timeout_is_never() {
        let mut h = PowerHonor::default();
        let base = Instant::now();
        h.step(
            None,
            IdleAction::Suspend,
            LidAction::Nothing,
            true,
            None,
            base,
        );
        // A day of idle with Never set: still nothing (the safe default).
        assert!(h
            .step(
                None,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                None,
                base + Duration::from_secs(86_400)
            )
            .is_empty());
    }

    #[test]
    fn step_maps_the_lid_close_to_the_configured_action_and_debounces() {
        let mut h = PowerHonor::default();
        let now = Instant::now();

        // Closed at startup (never seen open) → no surprise action.
        assert!(h
            .step(
                None,
                IdleAction::Suspend,
                LidAction::Suspend,
                false,
                Some(LidState::Closed),
                now
            )
            .is_empty());
        // Open, then Closed → the configured Suspend fires once.
        assert!(h
            .step(
                None,
                IdleAction::Suspend,
                LidAction::Suspend,
                false,
                Some(LidState::Open),
                now
            )
            .is_empty());
        assert_eq!(
            h.step(
                None,
                IdleAction::Suspend,
                LidAction::Suspend,
                false,
                Some(LidState::Closed),
                now
            ),
            vec![PowerVerb::Suspend]
        );
        // Held closed → no loop.
        assert!(h
            .step(
                None,
                IdleAction::Suspend,
                LidAction::Suspend,
                false,
                Some(LidState::Closed),
                now
            )
            .is_empty());
        // An Unknown flap → no fire.
        assert!(h
            .step(
                None,
                IdleAction::Suspend,
                LidAction::Suspend,
                false,
                Some(LidState::Unknown),
                now
            )
            .is_empty());

        // Reopen + reclose with Lock configured → the Lock verb fires (CURTAIN-3
        // routes it to the in-process curtain, not logind — asserted at the tick).
        h.step(
            None,
            IdleAction::Suspend,
            LidAction::Lock,
            false,
            Some(LidState::Open),
            now,
        );
        assert_eq!(
            h.step(
                None,
                IdleAction::Suspend,
                LidAction::Lock,
                false,
                Some(LidState::Closed),
                now
            ),
            vec![PowerVerb::Lock]
        );

        // With Nothing configured, a close performs no verb at all.
        h.step(
            None,
            IdleAction::Suspend,
            LidAction::Nothing,
            false,
            Some(LidState::Open),
            now,
        );
        assert!(h
            .step(
                None,
                IdleAction::Suspend,
                LidAction::Nothing,
                false,
                Some(LidState::Closed),
                now
            )
            .is_empty());
    }

    #[test]
    fn a_desktop_with_no_lid_reading_never_fires() {
        // `lid: None` is the Absent probe (a desktop) — the honorer never acts.
        let mut h = PowerHonor::default();
        let now = Instant::now();
        for _ in 0..5 {
            assert!(h
                .step(
                    None,
                    IdleAction::Suspend,
                    LidAction::Suspend,
                    false,
                    None,
                    now
                )
                .is_empty());
        }
    }

    #[test]
    fn lid_and_idle_action_verbs_map_and_the_labels_read() {
        assert_eq!(LidAction::Suspend.verb(), Some(PowerVerb::Suspend));
        assert_eq!(LidAction::Lock.verb(), Some(PowerVerb::Lock));
        assert_eq!(LidAction::Nothing.verb(), None);
        assert_eq!(LidAction::Suspend.label(), "Suspend");
        assert_eq!(LidAction::Nothing.label(), "Do nothing");
        assert_eq!(LidAction::ALL.len(), 3);

        // The idle action mirrors the same {Suspend, Lock, Nothing} → verb map.
        assert_eq!(IdleAction::Suspend.verb(), Some(PowerVerb::Suspend));
        assert_eq!(IdleAction::Lock.verb(), Some(PowerVerb::Lock));
        assert_eq!(IdleAction::Nothing.verb(), None);
        assert_eq!(IdleAction::Lock.label(), "Lock");
        assert_eq!(IdleAction::Nothing.label(), "Do nothing");
        assert_eq!(IdleAction::ALL.len(), 3);
        assert_eq!(IdleAction::default(), IdleAction::Suspend);
    }
}
