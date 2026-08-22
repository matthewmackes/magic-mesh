//! Send-To entry-point routing.
//!
//! The live Files surface (`mde-files-egui`) constructs exactly two
//! [`SendToEntry`] values (Q33 leftover / retired 6-set lock):
//!
//!   1. Toolbar primary-action button (`SendToEntry::Toolbar`)
//!   2. Right-click context menu (`SendToEntry::ContextMenu`)
//!
//! Command-palette, drag-and-drop, details-panel, and bulk-select-bar
//! variants were never constructed by the live surface and are gone.
//! Re-introduce an arm only at a real call site.
//!
//! Both entry points dispatch through the same [`SendToRequest`] type
//! so the orchestrator sees one canonical shape regardless of where
//! the user clicked.
//!
//! Pure-data module — no GUI widgets here. The Files surface routes
//! a constructed request to the backend (or, in tests, to the
//! in-memory `DemoBackend`).

use std::path::PathBuf;

use crate::backend::{ConflictPolicy, Destination, SendMode};

/// Where in the UI the Send-To verb fired from. Live Files surface
/// constructs [`Toolbar`](Self::Toolbar) and
/// [`ContextMenu`](Self::ContextMenu) only (Q33 leftover).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendToEntry {
    /// Toolbar primary-action button.
    Toolbar,
    /// Right-click → "Send to…" context menu item.
    ContextMenu,
}

impl SendToEntry {
    /// Stable kebab-case identifier for the audit log + telemetry.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Toolbar => "toolbar",
            Self::ContextMenu => "context-menu",
        }
    }

    /// Every live entry point. Lock-checked by tests.
    #[must_use]
    pub fn all() -> &'static [SendToEntry] {
        &[SendToEntry::Toolbar, SendToEntry::ContextMenu]
    }
}

/// Canonical request shape. Every entry point builds one of
/// these + dispatches it through `Message::SendTo`. The reducer
/// pipes the request into `Backend::send_to` after pre-flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendToRequest {
    /// Sources (file paths). Toolbar and context-menu fill these
    /// from the focused row or the multi-select set.
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
    fn all_live_entry_points_listed() {
        assert_eq!(SendToEntry::all().len(), 2);
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
    fn live_entry_set_is_toolbar_and_context_menu() {
        let slugs: HashSet<&'static str> = SendToEntry::all().iter().map(|e| e.slug()).collect();
        for required in ["toolbar", "context-menu"] {
            assert!(slugs.contains(required), "missing live entry {required}");
        }
        for retired in [
            "command-palette",
            "drag-drop",
            "details-panel",
            "bulk-select-bar",
        ] {
            assert!(
                !slugs.contains(retired),
                "retired 6-set entry {retired} must stay gone"
            );
        }
    }
}
