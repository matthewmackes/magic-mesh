//! Pure-fn keyboard dispatch — interprets a key + modifier set
//! into a [`KeyAction`] the Iced reducer translates into a
//! [`crate::Message`].
//!
//! CB-1.2 lock: "Tab cycles sidebar → main pane; Ctrl+1..9 jumps
//! to group; Escape closes detail." Splitting interpretation off
//! from the Iced subscription keeps it testable without
//! constructing real `iced::keyboard::Key` values.

use crate::model::Group;

/// Logical key panes the Tab cycler walks through. Order is
/// load-bearing — `Sidebar → Main → Sidebar → ...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Main,
}

impl Pane {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::Main,
            Self::Main => Self::Sidebar,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        // Two-pane cycle ⇒ `prev == next`. Keeping both names
        // documents intent at call sites (Tab vs Shift-Tab).
        self.next()
    }
}

/// Modifier subset relevant to workbench shortcuts. `shift` +
/// `ctrl` are flagged independently so Ctrl+Shift+N (future
/// new-window) doesn't collide with Ctrl+N.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
}

impl Modifiers {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            ctrl: false,
            shift: false,
        }
    }

    #[must_use]
    pub const fn ctrl() -> Self {
        Self {
            ctrl: true,
            shift: false,
        }
    }

    #[must_use]
    pub const fn shift() -> Self {
        Self {
            ctrl: false,
            shift: true,
        }
    }
}

/// Compact key vocabulary the workbench cares about — Iced's
/// full [`iced::keyboard::Key`] is folded onto this in the
/// subscription layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Tab,
    Escape,
    Digit(u8),
    Other,
}

/// Outcome of pressing one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Move keyboard focus to the supplied pane (Tab / Shift-Tab).
    FocusPane(Pane),
    /// Jump straight to the named group (Ctrl+1..9). The active
    /// view changes to [`crate::View::Group(g)`].
    JumpToGroup(Group),
    /// Close the detail pane and return to the active group
    /// landing view (Escape).
    CloseDetail,
    /// Key isn't bound — caller should ignore.
    Ignored,
}

/// Interpret one key press.
#[must_use]
pub fn interpret_key(key: Key, mods: Modifiers, current_pane: Pane) -> KeyAction {
    match (key, mods) {
        (Key::Tab, m) if !m.ctrl => {
            // Tab without Ctrl cycles panes; Shift-Tab reverses.
            // Two-pane cycle means next == prev, but the call
            // shape stays clear for future N-pane growth.
            let target = if m.shift {
                current_pane.prev()
            } else {
                current_pane.next()
            };
            KeyAction::FocusPane(target)
        }
        (Key::Escape, _) => KeyAction::CloseDetail,
        (Key::Digit(n), m) if m.ctrl && (1..=9).contains(&n) => {
            // Group-hotkey table: Ctrl+1 → first sidebar section … Ctrl+7
            // → seventh. NAV-1 has seven sections (Overview, This Node,
            // Mesh, Fleet, Provisioning, Monitoring, System); NAV-1.2 retired
            // the hidden Desktop group. A digit past the last section is
            // ignored (no panic).
            let idx = (n - 1) as usize;
            Group::sidebar_groups()
                .get(idx)
                .copied()
                .map_or(KeyAction::Ignored, KeyAction::JumpToGroup)
        }
        _ => KeyAction::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_next_and_prev_are_inverses_in_two_pane_cycle() {
        assert_eq!(Pane::Sidebar.next(), Pane::Main);
        assert_eq!(Pane::Main.next(), Pane::Sidebar);
        assert_eq!(Pane::Sidebar.next().next(), Pane::Sidebar);
        assert_eq!(Pane::Main.prev(), Pane::Sidebar);
    }

    #[test]
    fn tab_cycles_to_main_from_sidebar() {
        assert_eq!(
            interpret_key(Key::Tab, Modifiers::none(), Pane::Sidebar),
            KeyAction::FocusPane(Pane::Main)
        );
    }

    #[test]
    fn shift_tab_reverses_cycle() {
        assert_eq!(
            interpret_key(Key::Tab, Modifiers::shift(), Pane::Main),
            KeyAction::FocusPane(Pane::Sidebar)
        );
    }

    #[test]
    fn escape_closes_detail() {
        assert_eq!(
            interpret_key(Key::Escape, Modifiers::none(), Pane::Main),
            KeyAction::CloseDetail
        );
        // Escape ignores modifiers — Shift-Escape, Ctrl-Escape
        // all collapse to the same action so the muscle memory
        // works no matter what's held.
        assert_eq!(
            interpret_key(Key::Escape, Modifiers::ctrl(), Pane::Main),
            KeyAction::CloseDetail
        );
    }

    #[test]
    fn ctrl_digit_jumps_to_matching_sidebar_group() {
        // NAV-1 — Ctrl+1..7 map to the seven sections; Ctrl+8/9 are ignored
        // (there is no eighth section after NAV-1.2 retired Desktop).
        let cases = [
            (1, Group::Dashboard),
            (2, Group::ThisNode),
            (3, Group::Mesh),
            (4, Group::Fleet),
            (5, Group::Provisioning),
            (6, Group::Monitoring),
            (7, Group::System),
        ];
        for (n, expected) in cases {
            assert_eq!(
                interpret_key(Key::Digit(n), Modifiers::ctrl(), Pane::Sidebar),
                KeyAction::JumpToGroup(expected),
                "Ctrl+{n} should land on {expected:?}",
            );
        }
        // Past the last section → no-op (no panic).
        assert_eq!(
            interpret_key(Key::Digit(8), Modifiers::ctrl(), Pane::Sidebar),
            KeyAction::Ignored,
        );
    }

    #[test]
    fn plain_digit_without_ctrl_ignored() {
        assert_eq!(
            interpret_key(Key::Digit(3), Modifiers::none(), Pane::Sidebar),
            KeyAction::Ignored
        );
    }

    #[test]
    fn ctrl_zero_ignored_no_group_for_it() {
        assert_eq!(
            interpret_key(Key::Digit(0), Modifiers::ctrl(), Pane::Sidebar),
            KeyAction::Ignored
        );
    }

    #[test]
    fn unrelated_key_ignored() {
        assert_eq!(
            interpret_key(Key::Other, Modifiers::ctrl(), Pane::Main),
            KeyAction::Ignored
        );
    }

    #[test]
    fn ctrl_tab_ignored_so_app_switcher_chord_passes_through() {
        // Reserving Ctrl+Tab for higher-level chord routing
        // (e.g. mde-panel's app switcher) — the workbench
        // shouldn't capture it.
        assert_eq!(
            interpret_key(Key::Tab, Modifiers::ctrl(), Pane::Sidebar),
            KeyAction::Ignored
        );
    }
}
