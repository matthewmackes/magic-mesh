//! Phase 1.3 — Selection + multi-select model.
//!
//! Pure-data state that the Iced view consults to decide which rows
//! render with the selected style. Three click modes:
//!
//!   * **plain click** — replace the selection with just this row;
//!     anchor + focus + the single-element set all move here.
//!   * **ctrl-click** — toggle this row in the selection. Anchor +
//!     focus move to the row whether the toggle added or removed it.
//!   * **shift-click** — extend the selection from the anchor to
//!     this row over the caller-supplied ordered row list. Focus
//!     moves to the row; anchor stays put so a chain of shift-clicks
//!     keeps growing/shrinking from the same anchor.
//!
//! Keyboard navigation (arrow + space + enter handlers in the panel)
//! also goes through this module so the data model stays
//! deterministic: every selection change is one method call away
//! from a unit test.
//!
//! The selection holds `String` row keys (the file's stable
//! identifier — name + parent-path digest by Phase 2.5). Rows
//! aren't cached here; the model only tracks identity, not
//! payload, so the view re-renders cleanly when the underlying
//! list changes.

use std::collections::HashSet;

/// Selection state. Default = empty (no focus, no anchor, no
/// selection).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Row that has the keyboard / focus ring.
    focused: Option<String>,
    /// Anchor for shift-click ranges. Moves on plain click +
    /// ctrl-click; stays put on shift-click.
    anchor: Option<String>,
    /// Full selected set.
    selected: HashSet<String>,
}

impl Selection {
    /// Construct an empty selection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Plain click — replace the selection with `key`.
    pub fn click(&mut self, key: impl Into<String>) {
        let key = key.into();
        self.selected.clear();
        self.selected.insert(key.clone());
        self.anchor = Some(key.clone());
        self.focused = Some(key);
    }

    /// Ctrl-click — toggle `key` in the selection. Focus + anchor
    /// move to the clicked row regardless of whether it ended up in
    /// or out.
    pub fn ctrl_click(&mut self, key: impl Into<String>) {
        let key = key.into();
        if self.selected.contains(&key) {
            self.selected.remove(&key);
        } else {
            self.selected.insert(key.clone());
        }
        self.anchor = Some(key.clone());
        self.focused = Some(key);
    }

    /// Shift-click — extend the selection from the anchor to `key`
    /// over `ordered_rows`. If there's no anchor yet, falls back to
    /// a plain click. The selection becomes exactly the inclusive
    /// range; rows outside the range that were previously selected
    /// get dropped (matches Finder + Files behaviour).
    pub fn shift_click(&mut self, key: impl Into<String>, ordered_rows: &[String]) {
        let key = key.into();
        let Some(anchor) = self.anchor.clone() else {
            self.click(key);
            return;
        };
        let Some(start) = ordered_rows.iter().position(|r| r == &anchor) else {
            // Anchor isn't in the visible row list any more; treat as
            // a plain click so the user isn't stuck on a stale state.
            self.click(key);
            return;
        };
        let Some(end) = ordered_rows.iter().position(|r| r == &key) else {
            // Clicked row isn't in the visible list — ignore.
            return;
        };
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.selected.clear();
        for r in &ordered_rows[lo..=hi] {
            self.selected.insert(r.clone());
        }
        // Anchor stays put — only focus moves.
        self.focused = Some(key);
    }

    /// Clear the entire selection. Focus + anchor also reset.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.focused = None;
        self.anchor = None;
    }

    /// `true` if `key` is currently selected.
    #[must_use]
    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.contains(key)
    }

    /// `true` if `key` has the focus ring.
    #[must_use]
    pub fn is_focused(&self, key: &str) -> bool {
        self.focused.as_deref() == Some(key)
    }

    /// Currently focused row, if any.
    #[must_use]
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Shift-click anchor, if any.
    #[must_use]
    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    /// Number of selected rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// `true` when no rows are selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Selected row keys, deterministically sorted. The order is the
    /// lexicographic key order; the view sorts visible rows by its
    /// own column comparator, but the bulk-action layer needs a
    /// stable iteration order so the audit log is reproducible.
    #[must_use]
    pub fn iter_sorted(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.selected.iter().map(String::as_str).collect();
        out.sort_unstable();
        out
    }

    /// Move focus to the next row in `ordered_rows` (wraps at the
    /// end). Used by the keyboard handler. If nothing's focused yet,
    /// focuses the first row.
    pub fn focus_next(&mut self, ordered_rows: &[String]) {
        if ordered_rows.is_empty() {
            return;
        }
        let next = match &self.focused {
            None => ordered_rows[0].clone(),
            Some(current) => {
                let pos = ordered_rows
                    .iter()
                    .position(|r| r == current)
                    .map_or(0, |i| (i + 1) % ordered_rows.len());
                ordered_rows[pos].clone()
            }
        };
        self.focused = Some(next);
    }

    /// Move focus to the previous row in `ordered_rows` (wraps at the
    /// start).
    pub fn focus_prev(&mut self, ordered_rows: &[String]) {
        if ordered_rows.is_empty() {
            return;
        }
        let prev = match &self.focused {
            None => ordered_rows[ordered_rows.len() - 1].clone(),
            Some(current) => {
                let pos = ordered_rows.iter().position(|r| r == current);
                let next_pos = match pos {
                    Some(0) => ordered_rows.len() - 1,
                    Some(i) => i - 1,
                    None => ordered_rows.len() - 1,
                };
                ordered_rows[next_pos].clone()
            }
        };
        self.focused = Some(prev);
    }

    /// Space-bar handler: toggle the focused row into / out of the
    /// selection. No-op when nothing's focused.
    pub fn toggle_focused(&mut self) {
        if let Some(focused) = self.focused.clone() {
            if self.selected.contains(&focused) {
                self.selected.remove(&focused);
            } else {
                self.selected.insert(focused.clone());
            }
            self.anchor = Some(focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn new_selection_is_empty_and_unfocused() {
        let s = Selection::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(s.focused().is_none());
        assert!(s.anchor().is_none());
    }

    #[test]
    fn plain_click_replaces_and_anchors() {
        let mut s = Selection::new();
        s.click("a");
        assert_eq!(s.len(), 1);
        assert!(s.is_selected("a"));
        assert_eq!(s.focused(), Some("a"));
        assert_eq!(s.anchor(), Some("a"));

        // Plain click on b clears a, focuses + anchors b.
        s.click("b");
        assert_eq!(s.len(), 1);
        assert!(!s.is_selected("a"));
        assert!(s.is_selected("b"));
        assert_eq!(s.focused(), Some("b"));
        assert_eq!(s.anchor(), Some("b"));
    }

    #[test]
    fn ctrl_click_toggles_in_then_out() {
        let mut s = Selection::new();
        s.ctrl_click("a");
        assert!(s.is_selected("a"));
        s.ctrl_click("b");
        assert!(s.is_selected("a"));
        assert!(s.is_selected("b"));
        // Toggle a out.
        s.ctrl_click("a");
        assert!(!s.is_selected("a"));
        assert!(s.is_selected("b"));
        // Focus + anchor track the most recent click whether on / off.
        assert_eq!(s.focused(), Some("a"));
        assert_eq!(s.anchor(), Some("a"));
    }

    #[test]
    fn shift_click_extends_range_forward() {
        let rows = rows(&["a", "b", "c", "d", "e"]);
        let mut s = Selection::new();
        s.click("b");
        s.shift_click("d", &rows);
        assert_eq!(s.len(), 3);
        assert!(s.is_selected("b"));
        assert!(s.is_selected("c"));
        assert!(s.is_selected("d"));
        assert!(!s.is_selected("a"));
        assert!(!s.is_selected("e"));
        // Anchor stays at the original click; focus moves to the
        // shift target.
        assert_eq!(s.anchor(), Some("b"));
        assert_eq!(s.focused(), Some("d"));
    }

    #[test]
    fn shift_click_extends_range_backward() {
        let rows = rows(&["a", "b", "c", "d", "e"]);
        let mut s = Selection::new();
        s.click("d");
        s.shift_click("b", &rows);
        assert_eq!(s.len(), 3);
        assert!(s.is_selected("b"));
        assert!(s.is_selected("c"));
        assert!(s.is_selected("d"));
    }

    #[test]
    fn shift_click_with_no_anchor_falls_back_to_plain_click() {
        let rows = rows(&["a", "b", "c"]);
        let mut s = Selection::new();
        s.shift_click("b", &rows);
        assert_eq!(s.len(), 1);
        assert!(s.is_selected("b"));
        assert_eq!(s.anchor(), Some("b"));
    }

    #[test]
    fn shift_click_when_anchor_disappeared_falls_back_to_plain() {
        let mut s = Selection::new();
        s.click("ghost");
        let rows = rows(&["a", "b", "c"]);
        s.shift_click("b", &rows);
        assert_eq!(s.len(), 1);
        assert!(s.is_selected("b"));
    }

    #[test]
    fn shift_click_replaces_outside_range_rows() {
        // Finder / Files semantics: shift-click extends from the
        // *current* anchor, not the original click. ctrl-click moves
        // the anchor, so a subsequent shift-click extends from the
        // most-recent-clicked row.
        let rows = rows(&["a", "b", "c", "d", "e"]);
        let mut s = Selection::new();
        s.click("a"); // anchor=a
        s.ctrl_click("e"); // anchor=e, selected=[a,e]
        s.shift_click("c", &rows); // extends from e back to c
                                   // Range is c..e; selection becomes exactly {c, d, e}. a is
                                   // out of range and gets dropped.
        assert!(!s.is_selected("a"), "out-of-range a must be dropped");
        assert!(!s.is_selected("b"));
        assert!(s.is_selected("c"));
        assert!(s.is_selected("d"));
        assert!(s.is_selected("e"));
    }

    #[test]
    fn clear_resets_everything() {
        let mut s = Selection::new();
        s.click("a");
        s.ctrl_click("b");
        s.clear();
        assert!(s.is_empty());
        assert!(s.focused().is_none());
        assert!(s.anchor().is_none());
    }

    #[test]
    fn iter_sorted_is_deterministic() {
        let mut s = Selection::new();
        s.ctrl_click("zeta");
        s.ctrl_click("alpha");
        s.ctrl_click("middle");
        let got = s.iter_sorted();
        assert_eq!(got, vec!["alpha", "middle", "zeta"]);
    }

    #[test]
    fn focus_next_wraps_at_end() {
        let rows = rows(&["a", "b", "c"]);
        let mut s = Selection::new();
        s.focus_next(&rows);
        assert_eq!(s.focused(), Some("a"));
        s.focus_next(&rows);
        assert_eq!(s.focused(), Some("b"));
        s.focus_next(&rows);
        assert_eq!(s.focused(), Some("c"));
        s.focus_next(&rows);
        assert_eq!(s.focused(), Some("a"), "wraps to first");
    }

    #[test]
    fn focus_prev_wraps_at_start() {
        let rows = rows(&["a", "b", "c"]);
        let mut s = Selection::new();
        s.focus_prev(&rows);
        assert_eq!(s.focused(), Some("c"), "starts at last when none focused");
        s.focus_prev(&rows);
        assert_eq!(s.focused(), Some("b"));
        s.focus_prev(&rows);
        assert_eq!(s.focused(), Some("a"));
        s.focus_prev(&rows);
        assert_eq!(s.focused(), Some("c"), "wraps to last");
    }

    #[test]
    fn focus_next_handles_empty_row_list() {
        let mut s = Selection::new();
        s.focus_next(&[]);
        assert!(s.focused().is_none());
    }

    #[test]
    fn toggle_focused_is_noop_with_no_focus() {
        let mut s = Selection::new();
        s.toggle_focused();
        assert!(s.is_empty());
    }

    #[test]
    fn toggle_focused_toggles_under_focus() {
        let rows = rows(&["a", "b"]);
        let mut s = Selection::new();
        s.focus_next(&rows);
        s.toggle_focused();
        assert!(s.is_selected("a"));
        s.toggle_focused();
        assert!(!s.is_selected("a"));
    }

    #[test]
    fn focus_next_resumes_from_start_when_focus_not_in_list() {
        // Stale focus from a previous list — when the new list
        // doesn't contain it, focus jumps to the first row of the
        // new list instead of staying stuck on the ghost.
        let rows = rows(&["x", "y", "z"]);
        let mut s = Selection::new();
        s.click("ghost");
        s.focus_next(&rows);
        assert_eq!(s.focused(), Some("x"));
    }

    #[test]
    fn empty_selection_has_no_anchor_after_clear() {
        let mut s = Selection::new();
        s.click("a");
        s.shift_click("b", &rows(&["a", "b"]));
        s.clear();
        assert!(s.anchor().is_none());
    }
}
