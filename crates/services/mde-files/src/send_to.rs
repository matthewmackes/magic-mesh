//! Phase 3.1 — Send-To entry-point routing.
//!
//! The Send-To verb is reachable from six entry points (Q3.1
//! lock):
//!
//!   1. Toolbar primary-action button (`SendToEntry::Toolbar`)
//!   2. Right-click context menu (`SendToEntry::ContextMenu`)
//!   3. Command palette (`SendToEntry::CommandPalette`) — keyboard
//!      shortcut: Ctrl/Cmd + K
//!   4. Drag-and-drop onto a sidebar peer
//!      (`SendToEntry::DragDrop`)
//!   5. Details panel send-button (`SendToEntry::DetailsPanel`)
//!   6. Bulk-select action bar (`SendToEntry::BulkSelectBar`) —
//!      lit when `Selection::len() > 1`
//!
//! All six entry points dispatch through the same
//! [`SendToRequest`] type so the orchestrator sees one canonical
//! shape regardless of where the user clicked.
//!
//! Pure-data module — no Iced widgets here. The
//! `MdeFiles::update` reducer routes `Message::SendTo(req)` to
//! the backend (or, in tests, to the in-memory `DemoBackend`).

use std::path::PathBuf;

use crate::backend::{ConflictPolicy, Destination, SendMode};

/// Where in the UI the Send-To verb fired from. Locked 6-set
/// per the Phase 3.1 design lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendToEntry {
    /// Toolbar primary-action button.
    Toolbar,
    /// Right-click → "Send to…" context menu item.
    ContextMenu,
    /// Command palette (Ctrl/Cmd+K).
    CommandPalette,
    /// Drag-and-drop onto a sidebar peer card.
    DragDrop,
    /// Details-panel "Send" button.
    DetailsPanel,
    /// Bulk-select action bar (visible when multi-select set
    /// has > 1 row).
    BulkSelectBar,
}

impl SendToEntry {
    /// Stable kebab-case identifier for the audit log + telemetry.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Toolbar => "toolbar",
            Self::ContextMenu => "context-menu",
            Self::CommandPalette => "command-palette",
            Self::DragDrop => "drag-drop",
            Self::DetailsPanel => "details-panel",
            Self::BulkSelectBar => "bulk-select-bar",
        }
    }

    /// Every locked entry point. Lock-checked by tests.
    #[must_use]
    pub fn all() -> &'static [SendToEntry] {
        &[
            SendToEntry::Toolbar,
            SendToEntry::ContextMenu,
            SendToEntry::CommandPalette,
            SendToEntry::DragDrop,
            SendToEntry::DetailsPanel,
            SendToEntry::BulkSelectBar,
        ]
    }
}

/// Canonical request shape. Every entry point builds one of
/// these + dispatches it through `Message::SendTo`. The reducer
/// pipes the request into `Backend::send_to` after pre-flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendToRequest {
    /// Sources (file paths). Toolbar/context-menu/drag-drop fill
    /// these from the focused row or the multi-select set.
    pub sources: Vec<PathBuf>,
    /// Destination — peer / group / role / site (mirrors
    /// `backend::Destination`).
    pub destination: Destination,
    /// Send mode — Copy / Move / Sync / Deploy / Stage.
    pub mode: SendMode,
    /// Conflict resolution policy.
    pub conflict: ConflictPolicy,
    /// Where in the UI the verb fired from. Recorded in the
    /// audit log + the telemetry stream so the team can see
    /// which entry points users actually reach for.
    pub entry: SendToEntry,
}

impl SendToRequest {
    /// Convenience constructor with sensible defaults
    /// (`SendMode::Copy`, `ConflictPolicy::Ask`).
    #[must_use]
    pub fn copy_ask(sources: Vec<PathBuf>, destination: Destination, entry: SendToEntry) -> Self {
        Self {
            sources,
            destination,
            mode: SendMode::Copy,
            conflict: ConflictPolicy::Ask,
            entry,
        }
    }

    /// `true` when the request carries no sources — the reducer
    /// drops these silently (the orchestrator would reject them
    /// at the `sources` pre-flight check).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_six_entry_points_listed() {
        assert_eq!(SendToEntry::all().len(), 6);
    }

    #[test]
    fn entry_slugs_are_unique() {
        let mut seen: HashSet<&'static str> = HashSet::new();
        for e in SendToEntry::all() {
            assert!(seen.insert(e.slug()), "duplicate slug {:?}", e.slug());
        }
    }

    #[test]
    fn entry_slugs_are_kebab_case() {
        for e in SendToEntry::all() {
            let s = e.slug();
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "slug {s:?} must be kebab-case"
            );
            assert!(!s.is_empty());
            assert!(!s.starts_with('-'));
            assert!(!s.ends_with('-'));
        }
    }

    #[test]
    fn copy_ask_carries_defaults() {
        let r = SendToRequest::copy_ask(
            vec![PathBuf::from("/tmp/x")],
            Destination::Peer("pine".into()),
            SendToEntry::Toolbar,
        );
        assert_eq!(r.mode, SendMode::Copy);
        assert_eq!(r.conflict, ConflictPolicy::Ask);
        assert_eq!(r.entry, SendToEntry::Toolbar);
        assert_eq!(r.sources.len(), 1);
    }

    #[test]
    fn is_empty_returns_true_for_zero_sources() {
        let r = SendToRequest::copy_ask(
            vec![],
            Destination::Peer("pine".into()),
            SendToEntry::ContextMenu,
        );
        assert!(r.is_empty());
    }

    #[test]
    fn locked_six_entry_set_matches_design() {
        let slugs: HashSet<&'static str> = SendToEntry::all().iter().map(|e| e.slug()).collect();
        for required in [
            "toolbar",
            "context-menu",
            "command-palette",
            "drag-drop",
            "details-panel",
            "bulk-select-bar",
        ] {
            assert!(slugs.contains(required), "missing locked entry {required}");
        }
    }
}
