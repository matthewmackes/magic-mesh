//! User preferences for theme, density, and accessibility variants.
//! Persisted to `~/.config/mde/preferences.toml`.
//!
//! Available behind the `serde` feature flag — the rest of
//! `mde-theme` stays dep-free.

use crate::accessibility::A11y;
use crate::density::Density;
use crate::theme::Theme;

/// Aggregated user preferences resolved at startup. Default
/// values track the lock survey: `Theme::Dark`, `Density::Comfortable`,
/// no accessibility variants on.
///
/// FRONTDOOR-14 — no longer `Copy`: the embedded [`FrontDoorPrefs`] carries a
/// `Vec` (the tile arrangement), so the bundle is `Clone` instead. Every consumer
/// loads a fresh `Preferences` and reads its (still-`Copy`) scalar fields, so
/// dropping `Copy` here is invisible to them — no call site copied the whole
/// bundle by value.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Preferences {
    /// Theme — defaults to Dark per Q6 (wizard asks first-launch).
    #[cfg_attr(feature = "serde", serde(default = "default_theme"))]
    pub theme: Theme,
    /// Density — defaults to Comfortable per Q26.
    #[cfg_attr(feature = "serde", serde(default))]
    pub density: Density,
    /// Accessibility variants — all off by default per UX-22.
    #[cfg_attr(feature = "serde", serde(default))]
    pub a11y: A11y,
    /// MOTION-CORE-3 — global motion controls (kill switch + speed scale).
    #[cfg_attr(feature = "serde", serde(default))]
    pub motion: MotionPrefs,
    /// FRONTDOOR-14 — the Front Door's own persisted prefs (the in-menu settings
    /// panel writes here): the Copilot proactivity policy + the tile arrangement
    /// (order / pin / hide). Defaults to a quiet, untouched arrangement so a fresh
    /// install reads exactly as before the settings panel existed.
    #[cfg_attr(feature = "serde", serde(default))]
    pub front_door: FrontDoorPrefs,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            density: Density::Comfortable,
            a11y: A11y::default(),
            motion: MotionPrefs::default(),
            front_door: FrontDoorPrefs::default(),
        }
    }
}

/// FRONTDOOR-14 — the Front Door's persisted preferences (`preferences.toml
/// [front_door]`), the in-menu settings panel's store (design Q48 in-menu
/// settings + Q79 settings-managed tile arrangement). Theme + density already
/// live on [`Preferences`] (the settings panel writes those too); this carries
/// the Front-Door-specific knobs that have no home elsewhere:
///
///   * `ai_proactive` — the Copilot **proactivity** policy (Q61 — moderate
///     proactivity is the design default; the operator can turn the proactive
///     suggestion cards off entirely). The Front Door reads this to gate whether
///     it surfaces the inline suggestion cards + on-tile badges.
///   * `tiles` — the per-tile **arrangement** (Q79): the operator's order, plus
///     which tiles are pinned (sorted first) or hidden (dropped from the grid).
///     Stored as a list of stable tile-id rules so a tile the operator never
///     touched keeps its seed position, and an arrangement made before a tile
///     existed still applies to the tiles it names.
///
/// `serde(default)` on every field means an older config (one predating the
/// settings panel) loads with the design defaults — proactivity on, no
/// arrangement overrides — exactly as the Front Door behaved before FD-14.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FrontDoorPrefs {
    /// Q61 — surface Copilot's proactive suggestion cards + on-tile badges? `true`
    /// (the design's moderate proactivity) by default; `false` silences them
    /// entirely (the operator opted out in Settings). Gates the GUI's rendering of
    /// the suggestion set — it never changes what the backend publishes.
    #[cfg_attr(feature = "serde", serde(default = "default_ai_proactive"))]
    pub ai_proactive: bool,
    /// Q79 — the operator's tile arrangement rules, in the order they should be
    /// laid out. Each entry names a tile by its stable id and carries its pin /
    /// hide flags. A tile not named here keeps its seed order after the named
    /// ones; a named-but-now-absent tile is simply ignored (the Front Door maps
    /// ids to live tiles). Empty = the untouched seed arrangement.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tiles: Vec<TileArrangement>,
}

impl Default for FrontDoorPrefs {
    fn default() -> Self {
        // The design defaults: moderate proactivity ON (Q61), no arrangement
        // overrides (the untouched seed grid) — exactly the pre-FD-14 behavior.
        Self {
            ai_proactive: true,
            tiles: Vec::new(),
        }
    }
}

/// FRONTDOOR-14 — one tile's arrangement rule (Q79): its stable id plus whether
/// the operator pinned it (sorted to the front) or hid it (dropped from the
/// grid). The order of these entries in [`FrontDoorPrefs::tiles`] is the
/// operator's chosen tile order; the flags layer pin/hide over that order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TileArrangement {
    /// The stable tile id this rule addresses — a widget tile's [`crate::Theme`]-
    /// independent key id (e.g. `"mesh_map"`) or a launcher's lowercased label
    /// (e.g. `"files"`). The Front Door owns the id ↔ tile mapping; this crate
    /// only stores + round-trips the string.
    pub id: String,
    /// Pinned tiles sort to the front of the grid (before un-pinned ones), in
    /// arrangement order. Defaults `false`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub pinned: bool,
    /// Hidden tiles are dropped from the grid entirely (still listed in Settings
    /// so the operator can un-hide them). Defaults `false`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub hidden: bool,
}

#[cfg(feature = "serde")]
fn default_ai_proactive() -> bool {
    true
}

/// MOTION-A11Y-2 — whether a given animation is *decorative* or *essential*.
///
/// Decorative motion is ornamental polish, safe to drop; essential motion is a
/// loading/progress/state cue the user must still be able to read. The consumer
/// tags each animation with its role so "decorative-off" and the system
/// reduce-motion preference can drop the merely pretty motion while always
/// preserving the state cues. Glue: the role is a hint the consumer already
/// knows — `mde-theme` never guesses it from a preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionRole {
    /// Pure polish — hover lifts, skeleton shimmer, panel/dialog slide-ins,
    /// pulses. Conveys *no* information that isn't also carried by a static
    /// token (colour/elevation/text), so it can be dropped for comfort.
    Decorative,
    /// A state cue — a spinner/progress indicator, a loading/refresh activity
    /// pulse, a focus ring, a success/error flash. Carries information, so it is
    /// kept even with decorative motion off (still subject to the master
    /// kill-switch + the reduce-motion ≤80 ms cap, which keep it legible).
    Essential,
}

impl MotionRole {
    /// `true` for [`MotionRole::Decorative`] — the motion that "decorative-off"
    /// and the system reduce-motion preference are allowed to drop.
    #[must_use]
    pub const fn is_decorative(self) -> bool {
        matches!(self, Self::Decorative)
    }
}

/// MOTION-CORE-3 / MOTION-A11Y-2 — global motion configuration
/// (`preferences.toml [motion]`).
///
/// A master kill switch, a speed multiplier, and a `decorative` comfort toggle —
/// the single place to disable/scale/trim all shell animation. Env vars
/// `MDE_MOTION_DISABLED` / `MDE_MOTION_SCALE` / `MDE_MOTION_DECORATIVE` override
/// the file (CI / headless / quick toggles), and the read-only system
/// reduce-motion hint (`MDE_SYSTEM_REDUCE_MOTION`, the OS/Cosmic preference
/// surfaced by the platform layer) trims decorative motion **without** overwriting
/// the user's authoritative local config (GUI-9: Cosmic exposes no such control,
/// so the local file always wins).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MotionPrefs {
    /// Master switch — `false` disables all animation; surfaces render the
    /// terminal (final) frame with no interpolation.
    #[cfg_attr(feature = "serde", serde(default = "default_motion_enabled"))]
    pub enabled: bool,
    /// Global speed multiplier: `2.0` = twice as fast (half duration), `0.5` =
    /// half speed (double duration). Clamped to a sane range on apply.
    #[cfg_attr(feature = "serde", serde(default = "default_motion_scale"))]
    pub speed_scale: f32,
    /// MOTION-A11Y-2 — keep *decorative* motion (hover lifts, shimmer,
    /// slide-ins, pulses)? `false` drops it for comfort while keeping every
    /// [`MotionRole::Essential`] loading/progress/state cue. Defaults `true`
    /// (full polish); the system reduce-motion hint flips the *effective* value
    /// to `false` without rewriting this persisted field.
    #[cfg_attr(feature = "serde", serde(default = "default_motion_decorative"))]
    pub decorative: bool,
}

impl Default for MotionPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            speed_scale: 1.0,
            decorative: true,
        }
    }
}

impl MotionPrefs {
    /// MOTION-CORE-3 — resolve a [`crate::motion::Motion`] preset against the
    /// global controls + reduce-motion. Disabled → a zero-duration terminal
    /// frame; reduce-motion → the Q32 ≤80 ms crossfade (via
    /// [`crate::motion::Motion::resolved`]); otherwise the duration is scaled by
    /// `speed_scale` (clamped to `0.1..=10.0`).
    #[must_use]
    pub fn apply(self, m: crate::motion::Motion, reduce_motion: bool) -> crate::motion::Motion {
        use crate::motion::{Easing, Motion};
        if !self.enabled {
            return Motion {
                duration: std::time::Duration::ZERO,
                easing: Easing::Linear,
                looping: false,
            };
        }
        let m = m.resolved(reduce_motion);
        if reduce_motion {
            return m; // already capped + de-looped
        }
        let scale = self.speed_scale.clamp(0.1, 10.0);
        Motion {
            duration: std::time::Duration::from_secs_f32(m.duration.as_secs_f32() / scale),
            ..m
        }
    }

    /// MOTION-A11Y-2 — is *decorative* motion currently on, folding in the
    /// read-only system reduce-motion hint? The persisted `decorative` field is
    /// the user's local choice; the system preference (`system_reduce_motion` —
    /// the OS/Cosmic signal surfaced by the platform layer) can only ever
    /// *trim* decorative polish, never re-enable it. Local config stays
    /// authoritative (GUI-9): turning decorative off in the file wins even if the
    /// system asks for full motion. Returns `false` when either source asks to
    /// drop the polish.
    #[must_use]
    pub const fn decorative_enabled(self, system_reduce_motion: bool) -> bool {
        self.decorative && !system_reduce_motion
    }

    /// MOTION-A11Y-2 — resolve a [`crate::motion::Motion`] preset against the
    /// global controls, the animation's [`MotionRole`], **and** the system
    /// reduce-motion hint. The single entry point a consumer routes its motion
    /// through so the acceptance holds:
    ///
    ///   * **decorative-off removes lifts/shimmer** — a [`MotionRole::Decorative`]
    ///     animation collapses to a zero-duration terminal frame (the consumer
    ///     renders the static end state; the standing colour/elevation token is
    ///     the cue) whenever [`decorative_enabled`](Self::decorative_enabled) is
    ///     `false` (local `decorative=false` **or** the system hint).
    ///   * **but keeps loading/progress/state cues** — a [`MotionRole::Essential`]
    ///     animation is *never* dropped by the decorative gate; it still resolves
    ///     through [`apply`](Self::apply), so it honours the master kill-switch,
    ///     the speed scale, and the reduce-motion ≤80 ms cap (which keep it
    ///     legible) but always remains present.
    ///
    /// `reduce_motion` is the per-user a11y reduce-motion preference (the Q32
    /// contract); `system_reduce_motion` is the OS/Cosmic system preference.
    /// Both narrow motion; neither is allowed to widen it.
    #[must_use]
    pub fn apply_role(
        self,
        m: crate::motion::Motion,
        role: MotionRole,
        reduce_motion: bool,
        system_reduce_motion: bool,
    ) -> crate::motion::Motion {
        use crate::motion::{Easing, Motion};
        // The decorative gate only ever drops *decorative* motion — an essential
        // state cue passes straight through to the kill-switch / reduce-motion
        // resolution below.
        if role.is_decorative() && !self.decorative_enabled(system_reduce_motion) {
            return Motion {
                duration: std::time::Duration::ZERO,
                easing: Easing::Linear,
                looping: false,
            };
        }
        // A system reduce-motion request also caps essential motion to the Q32
        // ≤80 ms crossfade (matching the per-user reduce-motion contract) so the
        // cue stays but never *moves* the surface.
        self.apply(m, reduce_motion || system_reduce_motion)
    }
}

#[cfg(feature = "serde")]
fn default_theme() -> Theme {
    Theme::Dark
}

#[cfg(feature = "serde")]
fn default_motion_enabled() -> bool {
    true
}

#[cfg(feature = "serde")]
fn default_motion_scale() -> f32 {
    1.0
}

#[cfg(feature = "serde")]
fn default_motion_decorative() -> bool {
    true
}

/// MOTION-A11Y-2 — the read-only system reduce-motion hint (the OS/Cosmic
/// preference).
///
/// Cosmic exposes no such toggle today (GUI-9), so the platform layer surfaces
/// whatever the host desktop reports — GNOME's `gtk-enable-animations`, the
/// freedesktop `prefers-reduced-motion` portal — by setting
/// `MDE_SYSTEM_REDUCE_MOTION` (`1`/`true`/`yes`, case-insensitive) before startup.
/// `mde-theme` stays pure (no gsettings/portal dependency), reading only the env
/// hook. This is advisory: it can *trim* decorative motion (see
/// [`MotionPrefs::apply_role`]) but never overrides the authoritative local
/// config. Defaults `false` (the system asks for nothing).
#[must_use]
pub fn system_reduce_motion() -> bool {
    std::env::var("MDE_SYSTEM_REDUCE_MOTION").is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        v == "1" || v == "true" || v == "yes"
    })
}

impl Preferences {
    /// Parse a TOML string. Missing fields fall back to defaults.
    /// Available behind the `serde` feature.
    #[cfg(feature = "serde")]
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Serialize to a TOML string. Available behind the `serde`
    /// feature.
    #[cfg(feature = "serde")]
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Load from the standard XDG path, falling back to defaults when
    /// the file is absent or malformed. `MDE_REDUCE_MOTION=1` in the
    /// environment overrides the file value — useful in CI and headless
    /// contexts. Available behind the `serde` feature.
    #[cfg(feature = "serde")]
    pub fn load() -> Self {
        let mut prefs = Self::xdg_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| Self::from_toml_str(&raw).ok())
            .unwrap_or_default();
        if std::env::var_os("MDE_REDUCE_MOTION").map_or(false, |v| v != "0") {
            prefs.a11y.reduce_motion = true;
        }
        // MOTION-CORE-3 — env overrides for the global motion controls.
        if std::env::var_os("MDE_MOTION_DISABLED").map_or(false, |v| v != "0") {
            prefs.motion.enabled = false;
        }
        if let Ok(s) = std::env::var("MDE_MOTION_SCALE") {
            if let Ok(f) = s.parse::<f32>() {
                if f > 0.0 {
                    prefs.motion.speed_scale = f;
                }
            }
        }
        // MOTION-A11Y-2 — env override for the decorative comfort toggle
        // (CI / headless / a quick "just give me the static end-state" knob).
        if std::env::var_os("MDE_MOTION_DECORATIVE").map_or(false, |v| v == "0") {
            prefs.motion.decorative = false;
        }
        prefs
    }

    /// Persist to the standard XDG path (creating `~/.config/mde/`),
    /// so a Workbench theme change survives restart (GUI-3).
    /// Available behind the `serde` feature.
    #[cfg(feature = "serde")]
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::xdg_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no XDG_CONFIG_HOME/HOME")
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let toml = self
            .to_toml_string()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        std::fs::write(path, toml)
    }

    /// Standard XDG path for the preferences file —
    /// `${XDG_CONFIG_HOME:-$HOME/.config}/mde/preferences.toml`.
    /// Returns `None` if neither `XDG_CONFIG_HOME` nor `HOME`
    /// is set (which would mean the process is misconfigured).
    pub fn xdg_path() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| {
                    let mut p = std::path::PathBuf::from(h);
                    p.push(".config");
                    p
                })
            })?;
        let mut p = base;
        p.push("mde");
        p.push("preferences.toml");
        Some(p)
    }
}

// Serde derives for the contained types.
#[cfg(feature = "serde")]
impl serde::Serialize for Theme {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.id())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Theme {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Theme::from_id(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown theme id: {s:?}; expected \"dark\" or \"light\""
            ))
        })
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Density {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.id())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Density {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Density::from_id(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown density id: {s:?}; expected \"compact\", \"comfortable\", or \"spacious\""
            ))
        })
    }
}

#[cfg(feature = "serde")]
#[derive(serde::Serialize, serde::Deserialize)]
struct A11ySerde {
    #[serde(default)]
    high_contrast: bool,
    #[serde(default)]
    colorblind_safe: bool,
    #[serde(default)]
    reduce_motion: bool,
}

#[cfg(feature = "serde")]
impl serde::Serialize for A11y {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let s = A11ySerde {
            high_contrast: self.high_contrast,
            colorblind_safe: self.colorblind_safe,
            reduce_motion: self.reduce_motion,
        };
        s.serialize(ser)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for A11y {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = A11ySerde::deserialize(de)?;
        Ok(A11y {
            high_contrast: s.high_contrast,
            colorblind_safe: s.colorblind_safe,
            reduce_motion: s.reduce_motion,
        })
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn motion_prefs_default_enabled_unity_scale() {
        let m = MotionPrefs::default();
        assert!(m.enabled);
        assert!((m.speed_scale - 1.0).abs() < f32::EPSILON);
        // MOTION-A11Y-2 — decorative polish is on by default (full motion).
        assert!(m.decorative);
    }

    // ── MOTION-A11Y-2 — decorative-off + system reduce-motion ───────────────

    #[test]
    fn decorative_off_drops_lifts_and_shimmer_keeps_state_cues() {
        // Acceptance: decorative-off removes lifts/shimmer (the ornamental
        // presets) but keeps loading/progress/state cues (the essential ones).
        use crate::motion::Motion;
        let prefs = MotionPrefs {
            decorative: false,
            ..MotionPrefs::default()
        };
        // A decorative animation (hover lift / shimmer / panel slide) collapses
        // to a zero-duration terminal frame — no motion at all.
        let lift = prefs.apply_role(Motion::hover(), MotionRole::Decorative, false, false);
        assert_eq!(
            lift.duration,
            std::time::Duration::ZERO,
            "decorative lift is dropped"
        );
        assert!(!lift.looping);
        // An essential loading/progress cue is KEPT — it still animates (only the
        // master kill-switch / reduce-motion can trim it, neither set here). The
        // duration round-trips through `apply`'s f32 speed-scale, so compare with
        // tolerance like `motion_prefs_apply_scales_duration`.
        let loading = prefs.apply_role(Motion::loading(), MotionRole::Essential, false, false);
        assert!(
            (loading.duration.as_secs_f32() - Motion::loading().duration.as_secs_f32()).abs()
                < 1e-4,
            "essential loading cue survives decorative-off (got {:?})",
            loading.duration
        );
        assert!(loading.looping, "the loading activity loop is preserved");
    }

    #[test]
    fn decorative_on_keeps_decorative_motion() {
        // The default (decorative on) leaves a decorative animation animating.
        use crate::motion::Motion;
        let prefs = MotionPrefs::default();
        let lift = prefs.apply_role(Motion::hover(), MotionRole::Decorative, false, false);
        assert_eq!(lift.duration, Motion::hover().duration);
    }

    #[test]
    fn system_reduce_motion_trims_decorative_but_local_config_stays_authoritative() {
        // Acceptance: respect the system reduce-motion preference, but local
        // config stays authoritative (GUI-9). The system hint can only *trim*
        // decorative motion; it never re-enables what the user turned off, and it
        // does not rewrite the persisted field.
        // decorative = true locally.
        let on = MotionPrefs::default();
        // System asks for reduced motion ⇒ effective decorative is off…
        assert!(!on.decorative_enabled(true), "system hint trims decorative");
        // …but the persisted local field is untouched (still true).
        assert!(on.decorative, "system hint never rewrites local config");
        // …and the system asking for FULL motion can't re-enable a locally-off
        // decorative preference — local config wins.
        let off = MotionPrefs {
            decorative: false,
            ..MotionPrefs::default()
        };
        assert!(
            !off.decorative_enabled(false),
            "local decorative=false wins over a permissive system pref"
        );
    }

    #[test]
    fn system_reduce_motion_caps_essential_motion_but_keeps_it() {
        // The system reduce-motion hint caps an essential cue to the Q32 ≤80 ms
        // crossfade (no surface movement) — but the cue is still present.
        use crate::motion::Motion;
        let prefs = MotionPrefs::default();
        let loading = prefs.apply_role(Motion::loading(), MotionRole::Essential, false, true);
        assert_eq!(loading.duration, std::time::Duration::from_millis(80));
        assert!(!loading.looping, "looping is dropped under reduce-motion");
        // …and the decorative version is dropped entirely under the same hint.
        let lift = prefs.apply_role(Motion::hover(), MotionRole::Decorative, false, true);
        assert_eq!(lift.duration, std::time::Duration::ZERO);
    }

    #[test]
    fn motion_prefs_round_trips_decorative_through_toml() {
        // The decorative toggle persists + reloads.
        let prefs = MotionPrefs {
            decorative: false,
            ..MotionPrefs::default()
        };
        let wrapped = Preferences {
            motion: prefs,
            ..Preferences::default()
        };
        let s = wrapped.to_toml_string().unwrap();
        let parsed: Preferences = toml::from_str(&s).unwrap();
        assert!(!parsed.motion.decorative);
        // An older config with no `decorative` key defaults to on.
        let legacy = "[motion]\nenabled = true\nspeed_scale = 1.0\n";
        let p: Preferences = toml::from_str(legacy).unwrap();
        assert!(
            p.motion.decorative,
            "a config predating the toggle defaults to full polish"
        );
    }

    #[test]
    fn system_reduce_motion_reads_the_env_hook() {
        // The platform layer surfaces the OS/Cosmic preference via the env hook;
        // mde-theme stays pure (no gsettings/portal dep) and only reads it.
        std::env::set_var("MDE_SYSTEM_REDUCE_MOTION", "1");
        assert!(
            system_reduce_motion(),
            "env=1 ⇒ system asks for reduced motion"
        );
        std::env::set_var("MDE_SYSTEM_REDUCE_MOTION", "TRUE");
        assert!(system_reduce_motion(), "case-insensitive true");
        std::env::set_var("MDE_SYSTEM_REDUCE_MOTION", "0");
        assert!(!system_reduce_motion(), "env=0 ⇒ no system request");
        std::env::remove_var("MDE_SYSTEM_REDUCE_MOTION");
        assert!(!system_reduce_motion(), "unset ⇒ default false");
    }

    #[test]
    fn motion_prefs_apply_disabled_is_terminal_frame() {
        // MOTION-CORE-3 — disabled ⇒ zero-duration (terminal frame), no loop.
        let off = MotionPrefs {
            enabled: false,
            speed_scale: 1.0,
            ..MotionPrefs::default()
        };
        let r = off.apply(crate::motion::Motion::loading(), false);
        assert_eq!(r.duration, std::time::Duration::ZERO);
        assert!(!r.looping);
    }

    #[test]
    fn motion_prefs_apply_scales_duration() {
        // speed 2.0 ⇒ half duration; 0.5 ⇒ double.
        let base = crate::motion::Motion::panel_mount().duration.as_secs_f32();
        let fast = MotionPrefs {
            enabled: true,
            speed_scale: 2.0,
            ..MotionPrefs::default()
        }
        .apply(crate::motion::Motion::panel_mount(), false);
        assert!((fast.duration.as_secs_f32() - base / 2.0).abs() < 1e-4);
        let slow = MotionPrefs {
            enabled: true,
            speed_scale: 0.5,
            ..MotionPrefs::default()
        }
        .apply(crate::motion::Motion::panel_mount(), false);
        assert!((slow.duration.as_secs_f32() - base * 2.0).abs() < 1e-4);
    }

    #[test]
    fn motion_prefs_apply_reduce_motion_caps() {
        // reduce-motion wins over the speed scale — caps to the 80 ms crossfade.
        let r = MotionPrefs {
            enabled: true,
            speed_scale: 2.0,
            ..MotionPrefs::default()
        }
        .apply(crate::motion::Motion::loading(), true);
        assert_eq!(r.duration, std::time::Duration::from_millis(80));
        assert!(!r.looping);
    }

    #[test]
    fn default_serializes_to_minimal_toml() {
        let prefs = Preferences::default();
        let s = prefs.to_toml_string().unwrap();
        assert!(s.contains("theme = \"dark\""));
        assert!(s.contains("density = \"comfortable\""));
    }

    #[test]
    fn round_trip_through_toml() {
        let prefs = Preferences {
            theme: Theme::Light,
            density: Density::Compact,
            a11y: A11y {
                high_contrast: true,
                colorblind_safe: false,
                reduce_motion: true,
            },
            motion: MotionPrefs::default(),
            front_door: FrontDoorPrefs::default(),
        };
        let s = prefs.to_toml_string().unwrap();
        let parsed: Preferences = toml::from_str(&s).unwrap();
        assert_eq!(prefs, parsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s = "theme = \"light\"\n";
        let p: Preferences = toml::from_str(s).unwrap();
        assert_eq!(p.theme, Theme::Light);
        assert_eq!(p.density, Density::Comfortable);
        assert!(!p.a11y.high_contrast);
    }

    #[test]
    fn invalid_theme_id_returns_error() {
        let s = "theme = \"sepia\"\n";
        let err = toml::from_str::<Preferences>(s);
        assert!(err.is_err());
    }

    // ── FRONTDOOR-14 — the Front Door prefs (AI policy + tile arrangement) ──────

    #[test]
    fn front_door_prefs_default_proactivity_on_no_arrangement() {
        // The design default (Q61 moderate proactivity) is ON; a fresh install has
        // no arrangement overrides (the untouched seed grid).
        let fd = FrontDoorPrefs::default();
        assert!(fd.ai_proactive, "proactivity defaults on (Q61)");
        assert!(fd.tiles.is_empty(), "no arrangement overrides by default");
    }

    #[test]
    fn front_door_prefs_round_trip_through_toml() {
        // The AI policy + a pin/hide/reorder arrangement persists and reloads.
        let prefs = Preferences {
            front_door: FrontDoorPrefs {
                ai_proactive: false,
                tiles: vec![
                    TileArrangement {
                        id: "alerts".to_string(),
                        pinned: true,
                        hidden: false,
                    },
                    TileArrangement {
                        id: "music".to_string(),
                        pinned: false,
                        hidden: true,
                    },
                ],
            },
            ..Preferences::default()
        };
        let s = prefs.to_toml_string().unwrap();
        let parsed: Preferences = toml::from_str(&s).unwrap();
        assert_eq!(prefs.front_door, parsed.front_door);
        assert!(!parsed.front_door.ai_proactive);
        assert_eq!(parsed.front_door.tiles.len(), 2);
        assert!(parsed.front_door.tiles[0].pinned);
        assert!(parsed.front_door.tiles[1].hidden);
    }

    #[test]
    fn front_door_prefs_legacy_config_defaults_to_pre_fd14_behavior() {
        // A config predating the settings panel (no [front_door] table) loads with
        // proactivity ON and no arrangement — exactly how the Front Door behaved
        // before FD-14.
        let legacy = "theme = \"dark\"\ndensity = \"comfortable\"\n";
        let p: Preferences = toml::from_str(legacy).unwrap();
        assert!(p.front_door.ai_proactive);
        assert!(p.front_door.tiles.is_empty());
    }

    #[test]
    fn front_door_tile_arrangement_flags_default_off() {
        // A `[[front_door.tiles]]` entry naming only an id defaults both flags off.
        let s = "[[front_door.tiles]]\nid = \"system\"\n";
        let p: Preferences = toml::from_str(s).unwrap();
        assert_eq!(p.front_door.tiles.len(), 1);
        assert_eq!(p.front_door.tiles[0].id, "system");
        assert!(!p.front_door.tiles[0].pinned);
        assert!(!p.front_door.tiles[0].hidden);
    }
}

// Note: `Preferences::xdg_path()` reads `$XDG_CONFIG_HOME` / `$HOME`.
// We don't unit-test it here because `std::env::set_var` is `unsafe`
// in Rust 2024 and this crate forbids unsafe (lib.rs). The function
// is small + deterministic; integration tests at the consumer
// (mde-workbench / mde-session) cover the real-world resolution.
