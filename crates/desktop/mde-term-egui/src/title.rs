//! Per-pane editable titles (TERM-12).
//!
//! A pane's title has three layers, in falling precedence:
//!
//! 1. a **user override** (a rename — wins, and persists until cleared);
//! 2. the **auto-derived** title the running command set — the engine surfaces
//!    the shell's OSC 0/2 title (`Event::Title`) as
//!    [`crate::engine::TermEvent::Title`], the canonical "title follows the
//!    running command" mechanism (no re-derivation, §6);
//! 3. a **fallback** (the pane's stable ordinal) when neither is set.
//!
//! Renaming runs an in-place edit buffer the pane chrome binds a text field to;
//! committing an empty rename clears the override (back to the derived/fallback
//! title), so a pane can never be stuck with an unnameable blank title.

/// A pane's title state.
#[derive(Clone, Debug)]
pub struct PaneTitle {
    /// The auto-derived title (the running command's OSC title), if any.
    derived: Option<String>,
    /// The user's rename, if any (wins over the derived title).
    overridden: Option<String>,
    /// The stable fallback (the pane ordinal) when nothing else is set.
    fallback: String,
    /// The live rename buffer while editing (`None` when not renaming).
    editing: Option<String>,
}

impl PaneTitle {
    /// A fresh title with only its `fallback` (the pane ordinal), no derived or
    /// overridden title yet.
    #[must_use]
    pub fn new(fallback: impl Into<String>) -> Self {
        Self {
            derived: None,
            overridden: None,
            fallback: fallback.into(),
            editing: None,
        }
    }

    /// The title to show — override, else derived, else fallback (blank layers
    /// are skipped).
    #[must_use]
    pub fn display(&self) -> &str {
        [self.overridden.as_deref(), self.derived.as_deref()]
            .into_iter()
            .flatten()
            .find(|s| !s.trim().is_empty())
            .unwrap_or(&self.fallback)
    }

    /// Whether the user has renamed this pane.
    #[must_use]
    pub const fn is_overridden(&self) -> bool {
        self.overridden.is_some()
    }

    /// Adopt an auto-derived title (the running command's OSC title). A blank
    /// title is ignored, so a program that clears its title falls back rather
    /// than showing an empty pane label.
    pub fn set_derived(&mut self, title: impl Into<String>) {
        let title = title.into();
        if title.trim().is_empty() {
            return;
        }
        self.derived = Some(title);
    }

    /// Clear the auto-derived title (the engine's `ResetTitle`).
    pub fn reset_derived(&mut self) {
        self.derived = None;
    }

    /// Set the user override outright (a committed rename). An all-blank name
    /// clears the override instead (back to the derived/fallback title).
    pub fn set_override(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.overridden = if name.trim().is_empty() {
            None
        } else {
            Some(name)
        };
    }

    /// Clear the user override (revert to the derived/fallback title).
    pub fn clear_override(&mut self) {
        self.overridden = None;
    }

    /// Whether a rename is in progress.
    #[must_use]
    pub const fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Begin a rename, seeding the buffer with the current display title.
    pub fn begin_edit(&mut self) {
        self.editing = Some(self.display().to_owned());
    }

    /// The live rename buffer (a text field binds to it), or `None` when not
    /// editing.
    pub const fn edit_buffer_mut(&mut self) -> Option<&mut String> {
        self.editing.as_mut()
    }

    /// A read-only view of the rename buffer.
    #[must_use]
    pub fn edit_buffer(&self) -> Option<&str> {
        self.editing.as_deref()
    }

    /// Commit the rename: the buffer becomes the override (an empty buffer clears
    /// it). A no-op when not editing.
    pub fn commit_edit(&mut self) {
        if let Some(name) = self.editing.take() {
            self.set_override(name);
        }
    }

    /// Abandon the rename, leaving the title untouched.
    pub fn cancel_edit(&mut self) {
        self.editing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_title_is_its_fallback() {
        let t = PaneTitle::new("1");
        assert_eq!(t.display(), "1");
        assert!(!t.is_overridden());
    }

    #[test]
    fn a_derived_title_follows_the_running_command() {
        let mut t = PaneTitle::new("1");
        t.set_derived("vim README.md");
        assert_eq!(t.display(), "vim README.md");
        // A blank derived title is ignored (falls back).
        t.set_derived("   ");
        assert_eq!(t.display(), "vim README.md");
        // ResetTitle drops back to the fallback.
        t.reset_derived();
        assert_eq!(t.display(), "1");
    }

    #[test]
    fn a_user_override_wins_over_the_derived_title() {
        let mut t = PaneTitle::new("1");
        t.set_derived("bash");
        t.set_override("build logs");
        assert_eq!(t.display(), "build logs");
        assert!(t.is_overridden());
        // Even a fresh derived title doesn't displace the rename.
        t.set_derived("tail -f");
        assert_eq!(t.display(), "build logs");
        // Clearing the override reverts to the (latest) derived title.
        t.set_override("");
        assert_eq!(t.display(), "tail -f");
        assert!(!t.is_overridden());
    }

    #[test]
    fn editing_seeds_commits_and_cancels() {
        let mut t = PaneTitle::new("1");
        t.set_derived("bash");
        t.begin_edit();
        assert!(t.is_editing());
        assert_eq!(t.edit_buffer(), Some("bash")); // seeded with the display

        // Type a new name and commit.
        *t.edit_buffer_mut().unwrap() = "deploy".to_owned();
        t.commit_edit();
        assert!(!t.is_editing());
        assert_eq!(t.display(), "deploy");

        // Editing then cancelling leaves the title untouched.
        t.begin_edit();
        *t.edit_buffer_mut().unwrap() = "scratch".to_owned();
        t.cancel_edit();
        assert_eq!(t.display(), "deploy");
    }
}
