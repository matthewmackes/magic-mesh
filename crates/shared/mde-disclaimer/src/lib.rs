//! Single source of truth for the Mackes Workstation Warning / Disclaimer /
//! Mission. The repo-root `DISCLAIMER.md` is embedded at build time via
//! `include_str!`, so the shell's About surfaces, the installer, and the daemon
//! banner all render byte-identical text that can never drift from the docs —
//! edit `DISCLAIMER.md` and every consumer updates on the next build.
//!
//! Deliberately toolkit-free (no iced / GUI dep): any crate — shell, installer,
//! daemon — can depend on this without pulling a GUI stack. Rendering helpers
//! that need a toolkit live in the consuming crate (e.g. the shell's
//! `disclaimer::view`).

#![forbid(unsafe_code)]

/// The full Warning/Disclaimer/Mission text (Markdown), embedded from the
/// canonical repo-root `DISCLAIMER.md`.
pub const TEXT: &str = include_str!("../../../../DISCLAIMER.md");

/// Split the text into `(title, body)`: the heading (first markdown `#` line,
/// stripped) over the remaining paragraphs — so a surface can show a bold title
/// above the body. Falls back to `("Disclaimer", TEXT)` if there is no newline.
#[must_use]
pub fn split() -> (&'static str, &'static str) {
    match TEXT.split_once('\n') {
        Some((head, body)) => (head.trim_start_matches('#').trim(), body.trim_start()),
        None => ("Disclaimer", TEXT),
    }
}

/// `true` when the embedded disclaimer is non-empty. The E8 RPM pre-flight gate
/// requires `DISCLAIMER.md` to exist and be non-empty before any build; a shipped
/// binary should never see this return `false`.
#[must_use]
pub fn is_present() -> bool {
    !TEXT.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disclaimer_is_embedded_and_complete() {
        // The canonical text is embedded and carries its mission + the key
        // "as is" / "at your own risk" warranty waivers (so a surface always
        // shows the real disclaimer, not an empty/placeholder string).
        assert!(is_present());
        assert!(TEXT.contains("Mackes Workstation"));
        assert!(TEXT.contains("educational, experimental, open-source"));
        assert!(TEXT.contains("provided “as is”"));
        assert!(TEXT.contains("Use Mackes Workstation at your own risk."));
        assert!(TEXT.len() > 1500, "disclaimer text looks truncated");
    }

    #[test]
    fn split_extracts_title_and_body() {
        let (title, body) = split();
        assert_eq!(
            title,
            "Mackes Workstation — Warning, Disclaimer, and Mission Statement"
        );
        assert!(body.starts_with("Mackes Workstation is an educational"));
    }
}
