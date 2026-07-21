//! Car Mode key → action bindings (AUTO-KEYMAP-MODEL).
//!
//! A physical USB keyboard mounted in the vehicle drives Auto Mode: each key is
//! assigned directly to a [`CarAction`] (jump to Nav, play/pause media, answer a
//! call, …). The binding map is operator-editable in Settings → Key Mapping and
//! persisted to `settings-car-keys.json`, so it survives restart.
//!
//! Unlike the compiled-in seat hotkey table (deliberately fixed, no persistence —
//! see `mde_seat::hotkeys`), Car bindings are **configurable**, so this is modeled
//! on the persisted `AppearanceConfig` subsystem: `client_data_dir()` +
//! atomic-temp-and-rename write + `serde` tolerance of a missing/drifted file.
//!
//! Keys are stored by a **stable label** (`"1"`, `"F1"`) rather than by
//! `egui::Key`, because `egui::Key` is not `serde` — the label vocabulary here is
//! the single source of truth the on-disk map round-trips through.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mde_egui::egui;
use serde::{Deserialize, Serialize};

/// The file (under the client data dir) the Car key bindings persist to.
const CAR_KEYS_CONFIG_FILE: &str = "settings-car-keys.json";

/// One action a Car-Mode key can be bound to.
///
/// Serialized by its `snake_case` name so the on-disk map is stable across
/// `Surface`/enum reordering (neither `Surface` nor the seat `HotkeyAction`
/// derive `serde`, so this enum owns the stable vocabulary the bindings persist).
/// The shell translates each variant into a concrete effect in `apply_car_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarAction {
    /// Jump to the Auto Mode home (the glanceable tile launcher).
    GoHome,
    /// Jump to Navigation (the Drive HUD).
    GoNav,
    /// Jump to Media / Music.
    GoMedia,
    /// Jump to Phone (Voice / calls).
    GoPhone,
    /// Jump to Communications (alerts + messages).
    GoComms,
    /// Jump to Vehicle telematics.
    GoVehicle,
    /// Toggle media play / pause on the active player.
    MediaPlayPause,
    /// Skip to the next track / chapter.
    MediaNext,
    /// Skip to the previous track / chapter.
    MediaPrev,
    /// Answer an incoming call.
    CallAnswer,
    /// Decline an incoming call / hang up the active call.
    CallHangup,
}

impl CarAction {
    /// Every action, in the order the Key Mapping settings page lists them.
    pub const ALL: [Self; 11] = [
        Self::GoHome,
        Self::GoNav,
        Self::GoMedia,
        Self::GoPhone,
        Self::GoComms,
        Self::GoVehicle,
        Self::MediaPlayPause,
        Self::MediaNext,
        Self::MediaPrev,
        Self::CallAnswer,
        Self::CallHangup,
    ];

    /// The human label shown in the Auto Mode HUD legend + the Key Mapping page.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GoHome => "Home",
            Self::GoNav => "Navigation",
            Self::GoMedia => "Media",
            Self::GoPhone => "Phone",
            Self::GoComms => "Comms",
            Self::GoVehicle => "Vehicle",
            Self::MediaPlayPause => "Play / Pause",
            Self::MediaNext => "Next Track",
            Self::MediaPrev => "Previous Track",
            Self::CallAnswer => "Answer Call",
            Self::CallHangup => "Hang Up / Decline",
        }
    }

    /// Whether this action is a surface jump (vs. a transport / call verb) — used
    /// by the HUD tile grid, which only surfaces the jump targets.
    #[must_use]
    pub const fn is_surface_jump(self) -> bool {
        matches!(
            self,
            Self::GoHome
                | Self::GoNav
                | Self::GoMedia
                | Self::GoPhone
                | Self::GoComms
                | Self::GoVehicle
        )
    }
}

/// The physical keys Car Mode allows binding — the digit row and the function
/// row: the big, findable-by-touch keys a USB keyboard exposes cleanly through
/// egui (media keys arrive on a separate evdev path and are out of scope here).
/// Returned in the order the Key Mapping page lays them out.
#[must_use]
pub fn bindable_keys() -> Vec<egui::Key> {
    use egui::Key::{
        Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, F1, F10, F11, F12, F2, F3, F4,
        F5, F6, F7, F8, F9,
    };
    vec![
        Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, Num0, F1, F2, F3, F4, F5, F6, F7, F8,
        F9, F10, F11, F12,
    ]
}

/// The stable on-disk / display label for a bindable key (`"1"`, `"F1"`). Only
/// the [`bindable_keys`] set has a label; anything else is `None`.
#[must_use]
pub fn key_label(key: egui::Key) -> Option<&'static str> {
    use egui::Key::{
        Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, F1, F10, F11, F12, F2, F3, F4,
        F5, F6, F7, F8, F9,
    };
    Some(match key {
        Num1 => "1",
        Num2 => "2",
        Num3 => "3",
        Num4 => "4",
        Num5 => "5",
        Num6 => "6",
        Num7 => "7",
        Num8 => "8",
        Num9 => "9",
        Num0 => "0",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        _ => return None,
    })
}

/// Inverse of [`key_label`] — the `egui::Key` for a stable label, if bindable.
#[must_use]
pub fn key_from_label(label: &str) -> Option<egui::Key> {
    bindable_keys()
        .into_iter()
        .find(|k| key_label(*k) == Some(label))
}

/// The operator-editable Car-Mode key→action map, persisted to
/// `settings-car-keys.json`. Keyed by the stable key label so it round-trips
/// through JSON without depending on `egui::Key`'s (absent) `serde`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CarKeyBindings {
    bindings: BTreeMap<String, CarAction>,
}

impl Default for CarKeyBindings {
    fn default() -> Self {
        Self::defaults()
    }
}

impl CarKeyBindings {
    /// The factory driver layout: the digit + function rows mapped to the six
    /// Auto apps, plus play/pause + answer/hang-up — a usable set before any
    /// rebinding. Both a digit key and the aligned function key hit the same app
    /// (`1`/`F1` → Nav, …) so either row works from muscle memory.
    #[must_use]
    pub fn defaults() -> Self {
        use egui::Key::{Num1, Num2, Num3, Num4, Num5, Num6, F1, F2, F3, F4, F5, F6, F7, F8, F9};
        let mut bindings = BTreeMap::new();
        let mut set = |key: egui::Key, action: CarAction| {
            if let Some(l) = key_label(key) {
                bindings.insert(l.to_string(), action);
            }
        };
        for (num, fkey, action) in [
            (Num1, F1, CarAction::GoNav),
            (Num2, F2, CarAction::GoMedia),
            (Num3, F3, CarAction::GoPhone),
            (Num4, F4, CarAction::GoComms),
            (Num5, F5, CarAction::GoVehicle),
            (Num6, F6, CarAction::GoHome),
        ] {
            set(num, action);
            set(fkey, action);
        }
        set(F7, CarAction::MediaPlayPause);
        set(F8, CarAction::CallAnswer);
        set(F9, CarAction::CallHangup);
        Self { bindings }
    }

    /// The action bound to `key`, if any (and only for a bindable key).
    #[must_use]
    pub fn action_for(&self, key: egui::Key) -> Option<CarAction> {
        let label = key_label(key)?;
        self.bindings.get(label).copied()
    }

    /// Bind `key` to `action`, replacing any prior binding on that key. A no-op
    /// for a non-bindable key.
    pub fn set(&mut self, key: egui::Key, action: CarAction) {
        if let Some(l) = key_label(key) {
            self.bindings.insert(l.to_string(), action);
        }
    }

    /// Remove any binding on `key`.
    pub fn clear(&mut self, key: egui::Key) {
        if let Some(l) = key_label(key) {
            self.bindings.remove(l);
        }
    }

    /// The binding of every bindable key (`None` = unbound), in page order — the
    /// row model the Key Mapping settings grid renders.
    #[must_use]
    pub fn rows(&self) -> Vec<(egui::Key, Option<CarAction>)> {
        bindable_keys()
            .into_iter()
            .map(|k| (k, self.action_for(k)))
            .collect()
    }

    /// Reset to the factory layout (does not persist — the caller saves).
    pub fn reset(&mut self) {
        *self = Self::defaults();
    }

    /// The default bindings path (`<client-data-dir>/settings-car-keys.json`), or
    /// `None` in a headless context — mirrors `AppearanceConfig`.
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(CAR_KEYS_CONFIG_FILE))
    }

    /// Load from `path`, folding a missing / malformed file to the defaults
    /// (never fatal).
    fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

    /// Load from the default path (the factory layout when absent / unresolvable).
    #[must_use]
    pub fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` (atomic temp + rename, like `AppearanceConfig`).
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persist to the default path (a silent no-op when no data dir resolves).
    pub fn save(&self) {
        if let Some(path) = Self::default_path() {
            let _ = self.save_to(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_the_driver_layout() {
        let b = CarKeyBindings::defaults();
        // Digit + function rows both reach the six Auto apps.
        assert_eq!(b.action_for(egui::Key::Num1), Some(CarAction::GoNav));
        assert_eq!(b.action_for(egui::Key::F1), Some(CarAction::GoNav));
        assert_eq!(b.action_for(egui::Key::Num5), Some(CarAction::GoVehicle));
        assert_eq!(b.action_for(egui::Key::Num6), Some(CarAction::GoHome));
        // Transport + call verbs on the upper function row.
        assert_eq!(b.action_for(egui::Key::F7), Some(CarAction::MediaPlayPause));
        assert_eq!(b.action_for(egui::Key::F8), Some(CarAction::CallAnswer));
        assert_eq!(b.action_for(egui::Key::F9), Some(CarAction::CallHangup));
        // An unbound bindable key + a non-bindable key are both None.
        assert_eq!(b.action_for(egui::Key::F12), None);
        assert_eq!(b.action_for(egui::Key::A), None);
    }

    #[test]
    fn set_clear_and_reset_round_trip() {
        let mut b = CarKeyBindings::defaults();
        b.set(egui::Key::F12, CarAction::MediaNext);
        assert_eq!(b.action_for(egui::Key::F12), Some(CarAction::MediaNext));
        // Rebinding an occupied key replaces it.
        b.set(egui::Key::Num1, CarAction::GoComms);
        assert_eq!(b.action_for(egui::Key::Num1), Some(CarAction::GoComms));
        b.clear(egui::Key::Num1);
        assert_eq!(b.action_for(egui::Key::Num1), None);
        // A non-bindable key is a no-op, not a panic.
        b.set(egui::Key::A, CarAction::GoHome);
        assert_eq!(b.action_for(egui::Key::A), None);
        b.reset();
        assert_eq!(b, CarKeyBindings::defaults());
    }

    #[test]
    fn json_round_trips_by_stable_label() {
        let mut b = CarKeyBindings::defaults();
        b.set(egui::Key::F10, CarAction::MediaPrev);
        let json = serde_json::to_string(&b).expect("serialize");
        // The on-disk shape is a flat label→action map (transparent), stable.
        assert!(json.contains("\"1\":\"go_nav\""));
        assert!(json.contains("\"F10\":\"media_prev\""));
        let back: CarKeyBindings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, b);
    }

    #[test]
    fn rows_cover_every_bindable_key_in_page_order() {
        let b = CarKeyBindings::defaults();
        let rows = b.rows();
        assert_eq!(rows.len(), bindable_keys().len());
        assert_eq!(rows.first().map(|(k, _)| *k), Some(egui::Key::Num1));
        // Every row's key is bindable (has a label).
        assert!(rows.iter().all(|(k, _)| key_label(*k).is_some()));
    }

    #[test]
    fn key_label_round_trips() {
        for key in bindable_keys() {
            let label = key_label(key).expect("bindable key has a label");
            assert_eq!(key_from_label(label), Some(key));
        }
        assert_eq!(key_label(egui::Key::A), None);
        assert_eq!(key_from_label("nope"), None);
    }
}
