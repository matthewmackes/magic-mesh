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
pub trait TextClipboard {
    /// Return the text that should be pasted now, if one is available.
    fn read_text(&mut self) -> Option<String>;

    /// Record text copied by egui.
    fn write_text(&mut self, text: &str);
}

/// Process-local text clipboard used by compatibility callers and DRM examples.
#[derive(Debug, Default)]
pub struct MemoryTextClipboard {
    text: String,
}

impl MemoryTextClipboard {
    /// Create an empty process-local clipboard.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            text: String::new(),
        }
    }
}

impl TextClipboard for MemoryTextClipboard {
    fn read_text(&mut self) -> Option<String> {
        (!self.text.is_empty()).then(|| self.text.clone())
    }

    fn write_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryTextClipboard, TextClipboard};

    #[test]
    fn memory_provider_round_trips_text() {
        let mut clipboard = MemoryTextClipboard::new();
        assert!(clipboard.read_text().is_none());
        clipboard.write_text("seat text");
        assert_eq!(clipboard.read_text().as_deref(), Some("seat text"));
    }
}
