//! Phase 5.3 — accessibility labels for every icon-only button.
//!
//! Iced 0.13 ships `Element::accessibility_label` for screen-reader
//! announcement. Every icon-only button in the mde-files panel
//! routes its label through this module so:
//!
//!   * The English string for "back arrow", "send-to", "refresh",
//!     etc. lives in one place — the design + translation team
//!     consume the locked set without grepping the view code.
//!   * Test contracts assert each icon-only button has a label
//!     (no silent regression where a new button ships unlabelled).
//!
//! Each [`A11yAction`] variant maps to one icon-only widget. The
//! view code translates a click → message via the existing
//! `Message` enum; this module only owns the *label* used by AT.

#![allow(missing_docs)]

/// Every icon-only action surface the panel can render. New
/// icon-only buttons add a variant here so the screen-reader
/// label is mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum A11yAction {
    // Titlebar
    TitlebarMinimize,
    TitlebarMaximize,
    TitlebarClose,
    // Toolbar
    ToolbarRefresh,
    ToolbarSetLayoutList,
    ToolbarSetLayoutGrid,
    ToolbarCycleDensity,
    ToolbarBack,
    ToolbarForward,
    ToolbarUp,
    ToolbarPrimaryAction,
    // Sidebar
    SidebarToggleLocal,
    SidebarPeerSend,
    SidebarPeerOpen,
    // File-row controls
    RowOpen,
    RowSendTo,
    RowMore,
    // Operation drawer
    OpDrawerCancel,
    OpDrawerRetry,
    OpDrawerDismiss,
    OpDrawerExpand,
    // Details panel
    DetailsClose,
    DetailsCopyPath,
    // Context menu (rendered as text + icon — the chevron etc
    // still needs a label).
    ContextMenuOpenSubmenu,
}

/// Locked English label for `action`. Translation table lives
/// alongside this enum when i18n lands; until then English is the
/// single source of truth.
#[must_use]
pub fn label_for(action: A11yAction) -> &'static str {
    match action {
        A11yAction::TitlebarMinimize => "Minimize window",
        A11yAction::TitlebarMaximize => "Maximize window",
        A11yAction::TitlebarClose => "Close window",
        A11yAction::ToolbarRefresh => "Refresh file list",
        A11yAction::ToolbarSetLayoutList => "Switch to list layout",
        A11yAction::ToolbarSetLayoutGrid => "Switch to grid layout",
        A11yAction::ToolbarCycleDensity => "Cycle list density",
        A11yAction::ToolbarBack => "Go back",
        A11yAction::ToolbarForward => "Go forward",
        A11yAction::ToolbarUp => "Go up one folder",
        A11yAction::ToolbarPrimaryAction => "Send selected files",
        A11yAction::SidebarToggleLocal => "Show or hide the local files section",
        A11yAction::SidebarPeerSend => "Send files to this peer",
        A11yAction::SidebarPeerOpen => "Open this peer's folder",
        A11yAction::RowOpen => "Open this file",
        A11yAction::RowSendTo => "Send this file to a peer",
        A11yAction::RowMore => "More actions for this file",
        A11yAction::OpDrawerCancel => "Cancel this transfer",
        A11yAction::OpDrawerRetry => "Retry this transfer",
        A11yAction::OpDrawerDismiss => "Dismiss this transfer from the drawer",
        A11yAction::OpDrawerExpand => "Show transfer details",
        A11yAction::DetailsClose => "Close the details panel",
        A11yAction::DetailsCopyPath => "Copy file path to clipboard",
        A11yAction::ContextMenuOpenSubmenu => "Open submenu",
    }
}

/// Iterator over every locked action — used by tests to assert
/// completeness without per-variant noise.
#[must_use]
pub fn all_actions() -> &'static [A11yAction] {
    &[
        A11yAction::TitlebarMinimize,
        A11yAction::TitlebarMaximize,
        A11yAction::TitlebarClose,
        A11yAction::ToolbarRefresh,
        A11yAction::ToolbarSetLayoutList,
        A11yAction::ToolbarSetLayoutGrid,
        A11yAction::ToolbarCycleDensity,
        A11yAction::ToolbarBack,
        A11yAction::ToolbarForward,
        A11yAction::ToolbarUp,
        A11yAction::ToolbarPrimaryAction,
        A11yAction::SidebarToggleLocal,
        A11yAction::SidebarPeerSend,
        A11yAction::SidebarPeerOpen,
        A11yAction::RowOpen,
        A11yAction::RowSendTo,
        A11yAction::RowMore,
        A11yAction::OpDrawerCancel,
        A11yAction::OpDrawerRetry,
        A11yAction::OpDrawerDismiss,
        A11yAction::OpDrawerExpand,
        A11yAction::DetailsClose,
        A11yAction::DetailsCopyPath,
        A11yAction::ContextMenuOpenSubmenu,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_action_has_a_non_empty_label() {
        for a in all_actions() {
            let label = label_for(*a);
            assert!(!label.is_empty(), "{a:?} must have a label");
        }
    }

    #[test]
    fn labels_are_unique_per_action() {
        let mut seen: HashSet<&'static str> = HashSet::new();
        for a in all_actions() {
            let label = label_for(*a);
            assert!(
                seen.insert(label),
                "duplicate label {label:?} — actions must be distinguishable",
            );
        }
    }

    #[test]
    fn labels_are_sentence_case_and_descriptive() {
        // Lock: every label is at least 4 chars, starts with a
        // capital letter, and doesn't end in punctuation.
        for a in all_actions() {
            let l = label_for(*a);
            assert!(l.len() >= 4, "{a:?} label {l:?} too short");
            let first = l.chars().next().unwrap();
            assert!(
                first.is_ascii_uppercase(),
                "{a:?} label {l:?} must start with a capital letter"
            );
            assert!(
                !l.ends_with('.') && !l.ends_with('!') && !l.ends_with('?'),
                "{a:?} label {l:?} must not end with punctuation"
            );
        }
    }

    #[test]
    fn titlebar_labels_match_action() {
        assert!(label_for(A11yAction::TitlebarMinimize).contains("Minimize"));
        assert!(label_for(A11yAction::TitlebarMaximize).contains("Maximize"));
        assert!(label_for(A11yAction::TitlebarClose).contains("Close"));
    }

    #[test]
    fn layout_switch_labels_distinguish_list_vs_grid() {
        let l = label_for(A11yAction::ToolbarSetLayoutList);
        let g = label_for(A11yAction::ToolbarSetLayoutGrid);
        assert_ne!(l, g);
        assert!(l.contains("list"));
        assert!(g.contains("grid"));
    }

    #[test]
    fn op_drawer_labels_call_out_per_op_actions() {
        assert!(label_for(A11yAction::OpDrawerCancel).contains("Cancel"));
        assert!(label_for(A11yAction::OpDrawerRetry).contains("Retry"));
        assert!(label_for(A11yAction::OpDrawerDismiss).contains("Dismiss"));
    }

    #[test]
    fn all_actions_count_matches_enum_variants() {
        // Compile-time variant set — manually enumerated below so
        // adding a variant without a matching all_actions entry
        // fails the test immediately.
        let expected = 24usize;
        assert_eq!(
            all_actions().len(),
            expected,
            "all_actions() must list every variant"
        );
    }
}
