//! Stable egui ids for the Console surface's addressable cells (routing + tests).
//!
//! Relocated verbatim from `console.rs` in the directory-module split (arch); the
//! parent reaches these via `use ids::*` and re-exports the one `pub(crate)` id
//! (`console_entry_id`) that `start_menu`'s embedding test addresses by path.

use super::*;

// ── stable ids (the dock's addressable-cell idiom, for routing + tests) ─────

/// A flat entry row's stable id. `pub(crate)` (the other stable-id functions
/// below stay module-private) — `start_menu`'s own test reads this one back
/// to click a specific row of the embedded content and prove the WIN7-2
/// embedding is reachable end-to-end, not just architecturally wired.
pub(crate) fn console_entry_id(flat: usize) -> egui::Id {
    egui::Id::new(("console-entry", flat))
}

/// A rail category row's stable id.
pub(super) fn console_rail_id(label: &str) -> egui::Id {
    egui::Id::new(("console-rail", label))
}

/// A group heading's stable id (display-only; tests read its settled rect to
/// prove the jump-scroll).
pub(super) fn console_heading_id(label: &str) -> egui::Id {
    egui::Id::new(("console-heading", label))
}

/// A rail Power row's stable id (CONSOLE-4).
pub(super) fn console_power_id(action: PowerAction) -> egui::Id {
    egui::Id::new(("console-power", action.label()))
}

/// The typed-arming echo field's stable id (the one field the stage owns).
pub(super) fn console_arming_field_id() -> egui::Id {
    egui::Id::new("console-arming-field")
}

/// The arming stage's Confirm row id (fires only once armed, §7).
pub(super) fn console_confirm_id() -> egui::Id {
    egui::Id::new("console-arming-confirm")
}

/// The arming stage's Cancel row id.
pub(super) fn console_cancel_id() -> egui::Id {
    egui::Id::new("console-arming-cancel")
}

/// A Custom row's remove-cross id (CONSOLE-4), by entry index.
pub(super) fn console_custom_remove_id(index: usize) -> egui::Id {
    egui::Id::new(("console-custom-remove", index))
}

/// The Custom add-form's name field id.
pub(super) fn console_custom_name_id() -> egui::Id {
    egui::Id::new("console-custom-name")
}

/// The Custom add-form's command field id.
pub(super) fn console_custom_command_id() -> egui::Id {
    egui::Id::new("console-custom-command")
}

/// The Custom add-form's Add row id (disabled on a blank draft).
pub(super) fn console_custom_add_id() -> egui::Id {
    egui::Id::new("console-custom-add")
}
