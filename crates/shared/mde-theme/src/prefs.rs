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
#[derive(Clone, Copy, Debug, PartialEq)]
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
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            density: Density::Comfortable,
            a11y: A11y::default(),
            motion: MotionPrefs::default(),
        }
    }
}

/// MOTION-CORE-3 — global motion configuration (`preferences.toml [motion]`): a
/// master kill switch + a speed multiplier. Single place to disable/scale all
/// shell animation; env vars `MDE_MOTION_DISABLED` / `MDE_MOTION_SCALE` override
/// the file (CI / headless / quick toggles).
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
}

impl Default for MotionPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            speed_scale: 1.0,
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
    }

    #[test]
    fn motion_prefs_apply_disabled_is_terminal_frame() {
        // MOTION-CORE-3 — disabled ⇒ zero-duration (terminal frame), no loop.
        let off = MotionPrefs {
            enabled: false,
            speed_scale: 1.0,
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
        }
        .apply(crate::motion::Motion::panel_mount(), false);
        assert!((fast.duration.as_secs_f32() - base / 2.0).abs() < 1e-4);
        let slow = MotionPrefs {
            enabled: true,
            speed_scale: 0.5,
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
}

// Note: `Preferences::xdg_path()` reads `$XDG_CONFIG_HOME` / `$HOME`.
// We don't unit-test it here because `std::env::set_var` is `unsafe`
// in Rust 2024 and this crate forbids unsafe (lib.rs). The function
// is small + deterministic; integration tests at the consumer
// (mde-workbench / mde-session) cover the real-world resolution.
