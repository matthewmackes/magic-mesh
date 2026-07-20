//! The unified launcher's persisted favourites (pins) store — WL-UX-005.
//!
//! Extracted from the retired legacy Start Menu when the duplicate launcher was
//! removed: the operator's pinned launcher surfaces are the ONE local capability
//! the old `start_menu.rs` still owned that the unified Front Door lacked, so it
//! moves here rather than being lost. The Front Door remains a pure view over
//! this store — it renders the `pinned_surfaces()` list and emits
//! `FrontDoorRequest::TogglePin`/`MovePin`, which `main.rs` drains straight back
//! into this single store. No second preference file, no second launcher.
//!
//! On-disk continuity is deliberate: the same file name (`start-menu.json`), the
//! same stable, label-independent wire ids, and the same no-duplicate ordered-set
//! semantics the Start Menu used, so an operator who pinned surfaces before the
//! removal keeps every pin across the upgrade.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::dock::Surface;

/// Per-seat launcher pin preferences, stored beside the shell's other
/// client-data JSON prefs. Kept as `start-menu.json` for on-disk continuity with
/// the removed legacy Start Menu (WL-UX-005) so existing pins survive the upgrade.
const LAUNCHER_PINS_FILE: &str = "start-menu.json";

/// Stable JSON id for a pinnable launcher surface. Kept independent of display
/// labels so the persisted preferences do not depend on UI copy.
fn surface_wire_id(surface: Surface) -> &'static str {
    match surface {
        Surface::Workbench => "workbench",
        Surface::MeshView => "mesh_view",
        Surface::Explorer => "explorer",
        Surface::Desktop => "desktop",
        Surface::InfraCode => "infra_code",
        Surface::Music => "music",
        Surface::Media => "media",
        Surface::Files => "files",
        Surface::Voice => "voice",
        Surface::Browser => "browser",
        Surface::Bookmarks => "bookmarks",
        Surface::MapsLocation => "maps_location",
        Surface::Terminal => "terminal",
        Surface::Editor => "editor",
        Surface::Chat => "chat",
        Surface::Phones => "phones",
        Surface::Communications => "communications",
        Surface::System => "system",
        Surface::Storage => "storage",
        Surface::About => "about",
        Surface::Timers => "timers",
    }
}

/// Parse the stable launcher pin id. Unknown ids are treated as drifted /
/// hand-edited data and ignored by the loader.
fn surface_from_wire_id(id: &str) -> Option<Surface> {
    match id.trim() {
        "workbench" => Some(Surface::Workbench),
        "mesh_view" => Some(Surface::MeshView),
        "explorer" => Some(Surface::Explorer),
        "desktop" => Some(Surface::Desktop),
        "infra_code" => Some(Surface::InfraCode),
        "music" => Some(Surface::Music),
        "media" => Some(Surface::Media),
        "files" => Some(Surface::Files),
        "voice" => Some(Surface::Voice),
        "browser" => Some(Surface::Browser),
        "bookmarks" => Some(Surface::Bookmarks),
        "maps_location" => Some(Surface::MapsLocation),
        "terminal" => Some(Surface::Terminal),
        "editor" => Some(Surface::Editor),
        "chat" => Some(Surface::Chat),
        "phones" => Some(Surface::Phones),
        "communications" => Some(Surface::Communications),
        "system" => Some(Surface::System),
        "storage" => Some(Surface::Storage),
        "about" => Some(Surface::About),
        "timers" => Some(Surface::Timers),
        _ => None,
    }
}

/// Only real navigable surfaces are pinnable; anything not in [`Surface::ALL`]
/// is rejected by the one mutator so drifted data can never enter the store.
fn tileable_surface(surface: Surface) -> bool {
    Surface::ALL.contains(&surface)
}

/// The ONE pin mutator — an ordered-set toggle holding the no-duplicate
/// invariant (pin order is render order). Non-tileable surfaces are ignored.
fn toggle_pin(pinned: &mut Vec<Surface>, surface: Surface) {
    if !tileable_surface(surface) {
        return;
    }
    if let Some(idx) = pinned.iter().position(|&s| s == surface) {
        pinned.remove(idx);
    } else {
        pinned.push(surface);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct LauncherPinPrefs {
    #[serde(default)]
    pinned: Vec<String>,
}

impl LauncherPinPrefs {
    fn from_pins(pinned: &[Surface]) -> Self {
        let mut out = Vec::new();
        for &surface in pinned {
            if tileable_surface(surface) && !out.iter().any(|id| id == surface_wire_id(surface)) {
                out.push(surface_wire_id(surface).to_string());
            }
        }
        Self { pinned: out }
    }

    fn into_pins(self) -> Vec<Surface> {
        let mut out = Vec::new();
        for id in self.pinned {
            let Some(surface) = surface_from_wire_id(&id) else {
                continue;
            };
            if tileable_surface(surface) && !out.contains(&surface) {
                out.push(surface);
            }
        }
        out
    }

    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(LAUNCHER_PINS_FILE))
    }

    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

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
}

/// The launcher's ordered operator favourites and their persistence. Owned by
/// the shell and read by the Front Door; the Front Door never mutates it
/// directly, it emits pin requests that `main.rs` routes into these mutators —
/// so the platform has exactly one pin store, not two diverging preference files.
#[derive(Debug)]
pub struct LauncherPins {
    /// Where persisted pins live. `None` keeps the store purely in-memory, which
    /// is what unit fixtures want.
    prefs_path: Option<PathBuf>,
    /// The ordered favourites (a `Vec`-as-ordered-set — pin order is render
    /// order; the no-duplicate invariant is held by the one mutator [`toggle_pin`]).
    pinned: Vec<Surface>,
}

impl Default for LauncherPins {
    fn default() -> Self {
        Self {
            prefs_path: None,
            pinned: Vec::new(),
        }
    }
}

impl LauncherPins {
    /// Load the real shell's persisted launcher pins from client data. Missing or
    /// malformed files fold to an empty in-memory-compatible store.
    pub(crate) fn load() -> Self {
        let prefs_path = LauncherPinPrefs::default_path();
        let pinned = prefs_path
            .as_deref()
            .map(LauncherPinPrefs::load_from)
            .unwrap_or_default()
            .into_pins();
        Self { prefs_path, pinned }
    }

    #[cfg(test)]
    fn load_from(path: PathBuf) -> Self {
        let pinned = LauncherPinPrefs::load_from(&path).into_pins();
        Self {
            prefs_path: Some(path),
            pinned,
        }
    }

    fn persist(&self) {
        if let Some(path) = &self.prefs_path {
            let _ = LauncherPinPrefs::from_pins(&self.pinned).save_to(path);
        }
    }

    /// Ordered operator favourites for launcher surfaces. The Front Door reads
    /// this as a display priority; this store owns pin mutation and persistence.
    pub(crate) fn pinned_surfaces(&self) -> &[Surface] {
        &self.pinned
    }

    /// Toggle a launcher surface in the persisted favourites set. Returns whether
    /// the set actually changed (a non-tileable surface is a no-op).
    pub(crate) fn toggle_surface_pin(&mut self, surface: Surface) -> bool {
        let before = self.pinned.clone();
        toggle_pin(&mut self.pinned, surface);
        let changed = self.pinned != before;
        if changed {
            self.persist();
        }
        changed
    }

    pub(crate) fn move_surface_pin_up(&mut self, surface: Surface) -> bool {
        let Some(idx) = self
            .pinned
            .iter()
            .position(|&candidate| candidate == surface)
        else {
            return false;
        };
        if idx == 0 {
            return false;
        }
        self.pinned.swap(idx - 1, idx);
        self.persist();
        true
    }

    pub(crate) fn move_surface_pin_down(&mut self, surface: Surface) -> bool {
        let Some(idx) = self
            .pinned
            .iter()
            .position(|&candidate| candidate == surface)
        else {
            return false;
        };
        if idx + 1 >= self.pinned.len() {
            return false;
        }
        self.pinned.swap(idx, idx + 1);
        self.persist();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::Surface;

    #[test]
    fn default_store_is_empty_and_in_memory() {
        let pins = LauncherPins::default();
        assert!(pins.pinned_surfaces().is_empty());
    }

    #[test]
    fn toggle_pin_is_an_ordered_no_duplicate_set() {
        let mut pins = LauncherPins::default();
        assert!(pins.toggle_surface_pin(Surface::Browser));
        assert!(pins.toggle_surface_pin(Surface::Terminal));
        assert_eq!(
            pins.pinned_surfaces(),
            &[Surface::Browser, Surface::Terminal]
        );
        // A second toggle removes it (set semantics), reported as a change.
        assert!(pins.toggle_surface_pin(Surface::Browser));
        assert_eq!(pins.pinned_surfaces(), &[Surface::Terminal]);
    }

    #[test]
    fn move_pin_reorders_within_bounds_only() {
        let mut pins = LauncherPins::default();
        pins.toggle_surface_pin(Surface::Browser);
        pins.toggle_surface_pin(Surface::Terminal);
        pins.toggle_surface_pin(Surface::Files);
        // Down from the top swaps 0<->1.
        assert!(pins.move_surface_pin_down(Surface::Browser));
        assert_eq!(
            pins.pinned_surfaces(),
            &[Surface::Terminal, Surface::Browser, Surface::Files]
        );
        // Up from the bottom swaps last two.
        assert!(pins.move_surface_pin_up(Surface::Files));
        assert_eq!(
            pins.pinned_surfaces(),
            &[Surface::Terminal, Surface::Files, Surface::Browser]
        );
        // Moving the top up / the bottom down / an absent surface is refused.
        assert!(!pins.move_surface_pin_up(Surface::Terminal));
        assert!(!pins.move_surface_pin_down(Surface::Browser));
        assert!(!pins.move_surface_pin_up(Surface::Music));
    }

    #[test]
    fn pins_persist_and_reload_across_stores_by_stable_wire_id() {
        let dir = std::env::temp_dir().join(format!(
            "mcnf-launcher-pins-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join("start-menu.json");
        {
            let mut pins = LauncherPins::load_from(path.clone());
            pins.toggle_surface_pin(Surface::Browser);
            pins.toggle_surface_pin(Surface::Terminal);
        }
        // A fresh store over the same file reloads the same ordered pins.
        let reloaded = LauncherPins::load_from(path.clone());
        assert_eq!(
            reloaded.pinned_surfaces(),
            &[Surface::Browser, Surface::Terminal]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wire_ids_round_trip_for_every_surface() {
        for surface in Surface::ALL.iter().copied() {
            assert_eq!(
                surface_from_wire_id(surface_wire_id(surface)),
                Some(surface),
                "wire id must round-trip for {surface:?}",
            );
        }
    }
}
