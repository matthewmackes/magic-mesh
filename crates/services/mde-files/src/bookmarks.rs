//! Native sidebar bookmarks — the user's saved places (E11.6, Q34–Q39).
//!
//! Reads the GTK `bookmarks` file (`$XDG_CONFIG_HOME/gtk-3.0/bookmarks`) that
//! every freedesktop file manager shares, so mde-files' sidebar shows the same
//! "saved places" the user already curated elsewhere. Each line is a URI with an
//! optional display label: `file:///home/mm/Projects Work projects`. Native
//! parsing, no `gio`/GTK dependency.

use std::path::PathBuf;

/// One sidebar bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// The raw URI (`file://…`, or a remote `sftp://…`/`smb://…`).
    pub uri: String,
    /// The local path, when the URI is a `file://` one (decoded). `None` for
    /// remote URIs (still shown, but not a local path).
    pub path: Option<PathBuf>,
    /// The optional display label after the URI; falls back to the basename.
    pub label: String,
}

/// Parse a GTK `bookmarks` file body. Blank lines are skipped; each non-blank
/// line is `<uri>[ <label>]`.
#[must_use]
pub fn parse(content: &str) -> Vec<Bookmark> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (uri, label) = match line.split_once(' ') {
                Some((u, l)) => (u, l.trim().to_string()),
                None => (line, String::new()),
            };
            let path = uri
                .strip_prefix("file://")
                .map(|p| PathBuf::from(percent_decode(p)));
            let label = if label.is_empty() {
                // fall back to the URI's last path segment
                uri.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .map(|s| percent_decode(s))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| uri.to_string())
            } else {
                label
            };
            Bookmark {
                uri: uri.to_string(),
                path,
                label,
            }
        })
        .collect()
}

/// The user's bookmarks file path: `$XDG_CONFIG_HOME/gtk-3.0/bookmarks`, or
/// `$HOME/.config/gtk-3.0/bookmarks`.
#[must_use]
pub fn bookmarks_file() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("gtk-3.0").join("bookmarks"))
}

/// The user's current bookmarks (empty when the file is absent/unreadable —
/// never an error; the sidebar just shows no saved places).
#[must_use]
pub fn user_bookmarks() -> Vec<Bookmark> {
    bookmarks_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map_or_else(Vec::new, |c| parse(&c))
}

/// Decode `%XX` percent-escapes in a URI segment back to a byte string.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uri_with_explicit_label() {
        let bm = parse("file:///home/mm/Projects Work Projects\n");
        assert_eq!(bm.len(), 1);
        assert_eq!(bm[0].uri, "file:///home/mm/Projects");
        assert_eq!(bm[0].path, Some(PathBuf::from("/home/mm/Projects")));
        assert_eq!(bm[0].label, "Work Projects");
    }

    #[test]
    fn label_falls_back_to_basename() {
        let bm = parse("file:///home/mm/Documents\n");
        assert_eq!(bm[0].label, "Documents");
        assert_eq!(bm[0].path, Some(PathBuf::from("/home/mm/Documents")));
    }

    #[test]
    fn percent_escapes_in_the_path_are_decoded() {
        let bm = parse("file:///home/mm/My%20Files\n");
        assert_eq!(bm[0].path, Some(PathBuf::from("/home/mm/My Files")));
        assert_eq!(bm[0].label, "My Files");
    }

    #[test]
    fn remote_uris_keep_the_uri_but_have_no_local_path() {
        let bm = parse("sftp://host/srv Remote Server\n");
        assert_eq!(bm[0].path, None);
        assert_eq!(bm[0].uri, "sftp://host/srv");
        assert_eq!(bm[0].label, "Remote Server");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let bm = parse("\nfile:///a\n\n  \nfile:///b\n");
        assert_eq!(bm.len(), 2);
        assert_eq!(bm[0].label, "a");
        assert_eq!(bm[1].label, "b");
    }
}
