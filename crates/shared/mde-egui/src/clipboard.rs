//! Text clipboard seam shared by the bare-DRM runner and its owning shell.
//!
//! The renderer owns shortcut translation, while the shell owns platform and mesh
//! policy. Keeping those responsibilities behind this tiny trait lets the DRM
//! runner work without a compositor or a dependency on the platform Bus.

/// Hard byte ceiling for one local text clipboard value.
///
/// This is the same 1 MiB ceiling used by the existing VDI clipboard relay. The
/// local seat truncates at a UTF-8 boundary before a provider sees the value, so
/// a provider cannot accidentally retain an unbounded platform output. The Bus
/// producer remains responsible for its existing content-id, source, timestamp,
/// and echo/dedup semantics.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;

/// Normalize line endings and cap text without allocating an unbounded result.
///
/// A CRLF pair is one LF, a lone CR is also an LF, and the result is cut only at
/// a UTF-8 character boundary. Keeping this helper in the shared seam ensures
/// reads from a remote provider and writes from egui have the same contract.
#[must_use]
pub(crate) fn normalize_and_bound_text(text: &str, max_bytes: usize) -> String {
    if max_bytes == 0 || text.is_empty() {
        return String::new();
    }

    let mut bounded = String::with_capacity(text.len().min(max_bytes));
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let normalized = if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                let _ = chars.next();
            }
            '\n'
        } else {
            ch
        };
        if bounded.len() + normalized.len_utf8() > max_bytes {
            break;
        }
        bounded.push(normalized);
    }
    bounded
}

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
        let text = normalize_and_bound_text(text, MAX_CLIPBOARD_TEXT_BYTES);
        if text.is_empty() {
            self.text = None;
            return;
        }

        self.text = Some(text);
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryTextClipboard, TextClipboard, MAX_CLIPBOARD_TEXT_BYTES};

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

    #[test]
    fn memory_provider_bounds_without_splitting_utf8() {
        let mut clipboard = MemoryTextClipboard::new();
        let prefix = "a".repeat(MAX_CLIPBOARD_TEXT_BYTES - 1);
        clipboard.write_text(&format!("{prefix}é"));

        let stored = clipboard.read_text().expect("bounded text is retained");
        assert_eq!(stored.len(), MAX_CLIPBOARD_TEXT_BYTES - 1);
        assert_eq!(stored, prefix);
        assert!(stored.is_char_boundary(stored.len()));
    }

    #[test]
    fn normalization_and_bound_are_shared_by_local_writes() {
        let mut clipboard = MemoryTextClipboard::new();
        clipboard.write_text("one\r\ntwo\rthree");
        assert_eq!(clipboard.read_text().as_deref(), Some("one\ntwo\nthree"));
    }
}
