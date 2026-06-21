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
/// MOTION-* — the live reduce-motion flag, resolved once from the persisted
/// preferences (same lazy-init contract as [`TOKENS`]), so a `view()` reading it
/// every frame never hits the filesystem.
static REDUCE_MOTION: RwLock<Option<bool>> = RwLock::new(None);

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

/// MOTION-* — the live reduce-motion preference. Lazily resolved from
/// `~/.config/mde/preferences.toml` (first read), cached process-wide so motion
/// consumers (the MOTION-NET-2 skeleton shimmer, etc.) can read it per frame
/// without a filesystem hit. Never panics (a poisoned lock falls back to the
/// preference-resolved value).
pub fn reduce_motion() -> bool {
    if let Ok(guard) = REDUCE_MOTION.read() {
        if let Some(rm) = *guard {
            return rm;
        }
    }
    let rm = Preferences::load().a11y.reduce_motion;
    if let Ok(mut guard) = REDUCE_MOTION.write() {
        *guard = Some(rm);
    }
    rm
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
}
