//! GUI-2 — the Workbench's live theme state.
//!
//! One process-wide [`Tokens`] bundle, resolved from the persisted
//! [`Preferences`] at first read and swapped atomically when the
//! Themes panel applies a change. Views pull [`palette()`] on every
//! render (iced's view model), so a swap repaints live — no restart,
//! no per-panel threading of the palette through 90+ call sites.

use std::sync::RwLock;

use mde_theme::{Density, Palette, Preferences, Theme, Tokens};

static TOKENS: RwLock<Option<Tokens>> = RwLock::new(None);

/// Resolve from the persisted preferences (first read only).
fn resolve_from_prefs() -> Tokens {
    let p = Preferences::load();
    Tokens::resolve(p.theme, p.density)
}

/// The live token bundle. Lazily initialized from
/// `~/.config/mde/preferences.toml`; never panics (a poisoned lock
/// falls back to the preference-resolved bundle).
pub fn tokens() -> Tokens {
    if let Ok(guard) = TOKENS.read() {
        if let Some(t) = *guard {
            return t;
        }
    }
    let t = resolve_from_prefs();
    if let Ok(mut guard) = TOKENS.write() {
        *guard = Some(t);
    }
    t
}

/// The live palette — what every `view()` reads (GUI-2).
pub fn palette() -> Palette {
    tokens().palette
}

/// MOTION-A11Y-1 — the live reduce-motion flag every animated surface reads
/// (skeletons, transitions, pulses) so motion is gated from one source. True if
/// EITHER the `MDE_REDUCE_MOTION` env override is set (`1`/`true`, case-insensitive
/// — the acceptance's "with `MDE_REDUCE_MOTION=1` no surface moves") OR the
/// persisted a11y preference asks for it. Defaults to `false` (motion on).
#[must_use]
pub fn reduce_motion() -> bool {
    if let Ok(v) = std::env::var("MDE_REDUCE_MOTION") {
        let v = v.trim().to_ascii_lowercase();
        if v == "1" || v == "true" || v == "yes" {
            return true;
        }
    }
    Preferences::load().a11y.reduce_motion
}

/// MOTION-A11Y-2 — the live "play decorative motion?" flag every surface reads
/// before animating a *non-essential* flourish (hover-lift, shimmer breathe,
/// selection-slide accent, staggered reveal). `false` drops those while essential
/// state cues (loading/progress/refresh, async transitions, focus, success/error)
/// keep animating. True (decorative on) unless the `MDE_MOTION_DECORATIVE=0` env
/// override is set OR the persisted `[motion] decorative = false` /
/// `[motion] enabled = false` (the kill switch also implies no decorative motion).
/// Mirrors [`reduce_motion`]; local config is authoritative (Cosmic exposes no
/// system signal — GUI-9).
#[must_use]
pub fn decorative_motion() -> bool {
    if let Ok(v) = std::env::var("MDE_MOTION_DECORATIVE") {
        if v.trim() == "0" {
            return false;
        }
    }
    Preferences::load().motion.shows_decorative()
}

/// Swap the live theme/density. The next render pass repaints with
/// the new palette. Persistence is the caller's job
/// ([`mde_theme::Preferences::save`]) so tests can swap freely.
pub fn set(theme: Theme, density: Density) {
    let t = Tokens::resolve(theme, density);
    if let Ok(mut guard) = TOKENS.write() {
        *guard = Some(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_swaps_the_live_palette() {
        set(Theme::Dark, Density::Comfortable);
        let dark_bg = palette().background;
        set(Theme::Gray90, Density::Comfortable);
        let g90_bg = palette().background;
        assert_ne!(dark_bg, g90_bg, "Gray 90 swap must repaint");
        assert_eq!(g90_bg, Palette::gray_90().background);
        // Restore the default for other tests sharing the global.
        set(Theme::Dark, Density::Comfortable);
    }

    #[test]
    fn reduce_motion_honors_the_env_override() {
        // MOTION-A11Y-1 — MDE_REDUCE_MOTION=1 forces reduce-motion regardless of
        // the persisted preference; clearing it falls back to the preference.
        std::env::set_var("MDE_REDUCE_MOTION", "1");
        assert!(reduce_motion(), "env=1 forces reduce-motion");
        std::env::set_var("MDE_REDUCE_MOTION", "TRUE");
        assert!(reduce_motion(), "env is case-insensitive");
        std::env::set_var("MDE_REDUCE_MOTION", "0");
        // env=0 → not forced; result is whatever the (test-default) preference is.
        let _ = reduce_motion();
        std::env::remove_var("MDE_REDUCE_MOTION");
    }

    #[test]
    fn decorative_motion_honors_the_env_override() {
        // MOTION-A11Y-2 — MDE_MOTION_DECORATIVE=0 forces decorative motion off
        // regardless of the persisted preference; clearing it falls back to prefs
        // (default decorative on).
        std::env::set_var("MDE_MOTION_DECORATIVE", "0");
        assert!(!decorative_motion(), "env=0 forces decorative motion off");
        std::env::remove_var("MDE_MOTION_DECORATIVE");
        // Default preference keeps decorative motion on.
        assert!(
            decorative_motion(),
            "default prefs keep decorative motion on"
        );
    }
}
