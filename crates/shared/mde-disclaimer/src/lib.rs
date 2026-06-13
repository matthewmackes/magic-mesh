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

// ---- AUD-5: the runtime pre-flight accept gate (§5) ------------------------
//
// §5 requires a runtime "I agree before use" consent, not just the build-time
// non-empty check + a display-only About panel. This records the operator's
// acceptance keyed to the disclaimer's content fingerprint, so a *material*
// change to DISCLAIMER.md re-prompts. Toolkit-free: the GUI accept screen lives
// in the consuming surface (the Workbench); headless flows use the
// `mackesd accept-disclaimer` CLI.

/// A stable hex fingerprint of the current disclaimer text. Acceptance is keyed
/// to this so editing the disclaimer invalidates a prior acceptance.
#[must_use]
pub fn fingerprint() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    TEXT.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Where the acceptance marker is written:
/// `$XDG_CONFIG_HOME/mde/disclaimer-accepted` (falling back to
/// `$HOME/.config/mde/…`). `None` when neither env var is set.
#[must_use]
pub fn acceptance_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("mde").join("disclaimer-accepted"))
}

/// `true` when the operator has accepted the *current* disclaimer (the marker
/// exists and records the matching [`fingerprint`]). A changed disclaimer or a
/// missing marker reads as not-accepted.
#[must_use]
pub fn is_accepted() -> bool {
    // Headless / CI / preview escape: automated renders + non-interactive runs
    // can't click Accept. `MDE_DISCLAIMER_ACCEPTED=1` pre-satisfies the gate
    // (the real install-time/first-run consent still applies on operator boxes).
    if std::env::var_os("MDE_DISCLAIMER_ACCEPTED").is_some_and(|v| v != "0") {
        return true;
    }
    acceptance_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|recorded| recorded.trim() == fingerprint())
}

/// Record the operator's acceptance of the current disclaimer (writes the
/// fingerprint to [`acceptance_path`]). Idempotent.
///
/// # Errors
///
/// I/O errors creating the config dir or writing the marker.
pub fn record_acceptance() -> std::io::Result<()> {
    let path = acceptance_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no XDG_CONFIG_HOME / HOME to record disclaimer acceptance",
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, fingerprint())
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
        assert!(TEXT.contains("Magic Mesh"));
        assert!(TEXT.contains("no-fixed-center"));
        assert!(TEXT.contains(r#"provided "as is""#));
        assert!(TEXT.contains("Use Magic Mesh at your own risk."));
        assert!(TEXT.len() > 1500, "disclaimer text looks truncated");
    }

    #[test]
    fn acceptance_round_trips_and_is_fingerprint_keyed() {
        // Isolate the config dir so the test never touches the real marker.
        let tmp = std::env::temp_dir().join(format!("mde-disc-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // SAFETY: single-threaded test; restore after.
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        std::env::remove_var("MDE_DISCLAIMER_ACCEPTED"); // ignore the CI escape here

        assert!(!is_accepted(), "fresh config = not accepted");
        record_acceptance().expect("record");
        assert!(is_accepted(), "recorded acceptance reads back");
        // A changed disclaimer (different fingerprint) invalidates it.
        std::fs::write(acceptance_path().unwrap(), "stale-fingerprint").unwrap();
        assert!(!is_accepted(), "fingerprint mismatch = re-prompt");

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn split_extracts_title_and_body() {
        let (title, body) = split();
        assert_eq!(
            title,
            "Magic Mesh — Warning, Disclaimer, and Mission Statement"
        );
        assert!(body.starts_with("Magic Mesh is an open-source"));
    }
}
