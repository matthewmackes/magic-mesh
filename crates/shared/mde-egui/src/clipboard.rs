//! Text clipboard seam shared by the bare-DRM runner and its owning shell.
//!
//! The renderer owns shortcut translation, while the shell owns platform and mesh
//! policy. Keeping those responsibilities behind this tiny trait lets the DRM
//! runner work without a compositor or a dependency on the platform Bus.

/// A text-only clipboard provider for the bare-DRM runner.
///
/// Implementations may refresh from an external source in [`Self::read_text`] and
/// publish a local copy in [`Self::write_text`]. Failures stay implementation-owned
/// so a temporarily unavailable mesh transport never breaks local input handling.
/// An implementation must return `None` when no text is available; callers use that
/// result to clear any cached paste value. Passing an empty string to
/// [`Self::write_text`] clears the provider.
pub trait TextClipboard {
    /// Return the text that should be pasted now, if one is available.
    fn read_text(&mut self) -> Option<String>;

    /// Record text copied by egui.
    fn write_text(&mut self, text: &str);
}

/// Process-local text clipboard used by compatibility callers and DRM examples.
#[derive(Debug, Default)]
pub struct MemoryTextClipboard {
    text: Option<String>,
}

impl MemoryTextClipboard {
    /// Create an empty process-local clipboard.
    #[must_use]
    pub const fn new() -> Self {
        Self { text: None }
    }
}

impl TextClipboard for MemoryTextClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn write_text(&mut self, text: &str) {
        if text.is_empty() {
            self.text = None;
            return;
        }

        // Keep the process-local provider on the same line-ending contract as
        // the DRM paste path. This prevents a local copy from changing shape
        // depending on which egui surface produced it.
        self.text = Some(text.replace("\r\n", "\n").replace('\r', "\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryTextClipboard, TextClipboard};

    #[test]
    fn memory_provider_round_trips_text() {
        let mut clipboard = MemoryTextClipboard::new();
        assert!(clipboard.read_text().is_none());
        clipboard.write_text("seat\r\ntext\rline");
        assert_eq!(clipboard.read_text().as_deref(), Some("seat\ntext\nline"));
    }

    #[test]
    fn empty_write_clears_the_available_text() {
        let mut clipboard = MemoryTextClipboard::new();
        clipboard.write_text("stale text");
        clipboard.write_text("");
        assert!(clipboard.read_text().is_none());
    }
}
