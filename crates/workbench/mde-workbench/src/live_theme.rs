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

/// MOTION-A11Y-1 — the COSMIC toolkit config name + reduce-motion key the
/// Workbench reads the system reduced-motion accessibility preference from.
///
/// `com.system76.CosmicTk` is the config libcosmic itself exposes for toolkit
/// preferences (density, fonts, window-button visibility). The pinned libcosmic
/// rev does NOT yet carry a reduce-motion field on `CosmicTk` (and cosmic-time's
/// auto-detect is an upstream TODO), so we read the key *directly* with
/// cosmic-config's untyped `get` instead of going through the typed
/// `CosmicTk` entry. Today that key is absent ⇒ [`cosmic_reduce_motion_signal`]
/// returns `None` and the local preference stays authoritative (design doc
/// GUI-9: "keep local config authoritative"); the instant COSMIC ships the key
/// under this name the signal lights up with no further wiring.
pub const COSMIC_TK_CONFIG_ID: &str = "com.system76.CosmicTk";
/// MOTION-A11Y-1 — config version of [`COSMIC_TK_CONFIG_ID`] (libcosmic
/// `CosmicTk::VERSION` == 1 at the pinned rev).
pub const COSMIC_TK_CONFIG_VERSION: u64 = 1;
/// MOTION-A11Y-1 — the candidate per-key entry names a COSMIC reduced-motion
/// preference could live under. We probe each (first hit wins) so the wiring is
/// robust to the exact name upstream lands on.
const COSMIC_REDUCE_MOTION_KEYS: &[&str] = &["reduce_motion", "reduced_motion"];

/// MOTION-A11Y-1 — read the system reduced-motion accessibility preference from
/// the COSMIC toolkit config, if COSMIC publishes one.
///
/// Returns `Some(true|false)` when a reduce-motion key is present in the
/// `com.system76.CosmicTk` config, or `None` when COSMIC doesn't expose it
/// (the current upstream state) or when there's no COSMIC config at all
/// (headless / non-COSMIC session). Never panics — any cosmic-config error maps
/// to `None`, so the caller falls back to the local preference.
#[must_use]
pub fn cosmic_reduce_motion_signal() -> Option<bool> {
    use cosmic::cosmic_config::{Config, ConfigGet};
    let config = Config::new(COSMIC_TK_CONFIG_ID, COSMIC_TK_CONFIG_VERSION).ok()?;
    COSMIC_REDUCE_MOTION_KEYS
        .iter()
        .find_map(|key| config.get::<bool>(key).ok())
}

/// MOTION-A11Y-1 — does the `MDE_REDUCE_MOTION` override force reduce-motion on?
/// `true` for any value other than `"0"` (the documented opt-out); `None`/unset
/// ⇒ `false`. Split out so [`resolve_reduce_motion`] is pure + unit-testable
/// without mutating the process environment (the crate forbids `unsafe`, so a
/// test can't call `env::set_var`).
#[must_use]
fn env_override_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v != "0")
}

/// MOTION-A11Y-1 — resolve the authoritative reduce-motion flag from the three
/// sources, in priority order:
///
/// 1. `MDE_REDUCE_MOTION` env override (`!= "0"` ⇒ on) — wins over everything,
///    for CI / headless / a quick local toggle.
/// 2. The COSMIC system signal (`cosmic_signal`), when COSMIC publishes one.
/// 3. The local `~/.config/mde/preferences.toml` `[a11y] reduce_motion`.
///
/// Pure (the env value + COSMIC signal are passed in) so the priority logic is
/// unit-testable without a COSMIC session or env mutation.
#[must_use]
pub fn resolve_reduce_motion_from(
    env_value: Option<&std::ffi::OsStr>,
    cosmic_signal: Option<bool>,
) -> bool {
    if env_override_on(env_value) {
        return true;
    }
    if let Some(system) = cosmic_signal {
        return system;
    }
    Preferences::load().a11y.reduce_motion
}

/// MOTION-A11Y-1 — [`resolve_reduce_motion_from`] reading the live
/// `MDE_REDUCE_MOTION` environment override.
#[must_use]
pub fn resolve_reduce_motion(cosmic_signal: Option<bool>) -> bool {
    resolve_reduce_motion_from(
        std::env::var_os("MDE_REDUCE_MOTION").as_deref(),
        cosmic_signal,
    )
}

/// MOTION-* / MOTION-A11Y-1 — the live reduce-motion preference. Lazily resolved
/// on first read from the env override + COSMIC system signal + the persisted
/// `~/.config/mde/preferences.toml`, cached process-wide so motion consumers (the
/// FEEDBACK hover-lift, the NET-2 skeleton shimmer, the TRANS-1 panel switch,
/// etc.) can read it per frame without a filesystem / config hit. The live value
/// is refreshed at runtime by [`set_reduce_motion`] when the Workbench's COSMIC
/// config subscription fires. Never panics (a poisoned lock falls back to the
/// resolved value).
pub fn reduce_motion() -> bool {
    if let Ok(guard) = REDUCE_MOTION.read() {
        if let Some(rm) = *guard {
            return rm;
        }
    }
    let rm = resolve_reduce_motion(cosmic_reduce_motion_signal());
    if let Ok(mut guard) = REDUCE_MOTION.write() {
        *guard = Some(rm);
    }
    rm
}

/// MOTION-A11Y-1 — swap the live reduce-motion flag. The next render pass
/// resolves all motion against the new value (movement collapses / restores
/// live, no restart). Called from the Workbench's COSMIC config subscription
/// when the system reduced-motion preference changes, and at boot to prime the
/// flag. Never panics (a poisoned lock is silently skipped — the next
/// [`reduce_motion`] read re-resolves).
pub fn set_reduce_motion(reduce_motion: bool) {
    if let Ok(mut guard) = REDUCE_MOTION.write() {
        *guard = Some(reduce_motion);
    }
}

/// MOTION-A11Y-1 — re-resolve the live reduce-motion flag from the current
/// env / COSMIC / local-preference state and store it. The runtime path the
/// COSMIC config subscription drives: when the toolkit config changes we read
/// the system signal afresh and update the cached flag. Returns the value it set
/// (so the caller can log / assert).
pub fn refresh_reduce_motion() -> bool {
    let rm = resolve_reduce_motion(cosmic_reduce_motion_signal());
    set_reduce_motion(rm);
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
    use std::ffi::OsStr;

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
    fn set_reduce_motion_swaps_the_live_flag() {
        // MOTION-A11Y-1 — the runtime setter the COSMIC config subscription
        // drives: flipping it is what every motion consumer reads next frame.
        set_reduce_motion(true);
        assert!(reduce_motion(), "set(true) ⇒ motion collapses");
        set_reduce_motion(false);
        assert!(!reduce_motion(), "set(false) ⇒ motion restored");
    }

    #[test]
    fn cosmic_signal_overrides_the_local_pref_when_present() {
        // MOTION-A11Y-1 — when COSMIC publishes a reduce-motion signal it wins
        // over the local preference (env override absent ⇒ `None`).
        assert!(
            resolve_reduce_motion_from(None, Some(true)),
            "COSMIC reduce_motion=on collapses motion"
        );
        assert!(
            !resolve_reduce_motion_from(None, Some(false)),
            "COSMIC reduce_motion=off restores motion (overrides a local on)"
        );
    }

    #[test]
    fn env_override_forces_reduce_motion_on() {
        // MOTION-A11Y-1 — MDE_REDUCE_MOTION wins over everything, even a COSMIC
        // signal that says motion is fine and the local pref.
        assert!(
            resolve_reduce_motion_from(Some(OsStr::new("1")), Some(false)),
            "env override beats a COSMIC 'motion ok' signal"
        );
        assert!(
            resolve_reduce_motion_from(Some(OsStr::new("1")), None),
            "env override beats the local pref"
        );
        // Any non-"0" value turns it on (matches Preferences::load).
        assert!(resolve_reduce_motion_from(Some(OsStr::new("yes")), None));
    }

    #[test]
    fn env_zero_is_the_opt_out_and_does_not_suppress_cosmic() {
        // MOTION-A11Y-1 — `MDE_REDUCE_MOTION=0` is the documented opt-out: it
        // must NOT force motion off, just decline to force it on — so a COSMIC
        // reduce-motion signal still wins.
        assert!(
            resolve_reduce_motion_from(Some(OsStr::new("0")), Some(true)),
            "MDE_REDUCE_MOTION=0 does not suppress a COSMIC reduce-motion signal"
        );
        assert!(
            !resolve_reduce_motion_from(Some(OsStr::new("0")), Some(false)),
            "=0 + COSMIC off ⇒ motion on"
        );
    }

    #[test]
    fn cosmic_signal_read_never_panics() {
        // MOTION-A11Y-1 — on a headless / non-COSMIC host the toolkit config
        // has no reduce-motion key, so the signal is None; on a real COSMIC
        // session it may be Some. The contract under test is only that the live
        // reader resolves without panicking regardless of session.
        let _ = cosmic_reduce_motion_signal();
        let _ = resolve_reduce_motion(None);
    }
}
