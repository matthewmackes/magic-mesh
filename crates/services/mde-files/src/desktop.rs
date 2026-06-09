//! Native default-application resolution — the "Open With" / open-default parity
//! op (E11.6, Q34–Q39).
//!
//! Resolves which installed application handles a file, the freedesktop way: a
//! MIME type → the `[Default Applications]` entry in the `mimeapps.list` chain →
//! the matching `applications/<id>.desktop` → its `Exec` line (field codes
//! expanded). Native parsing of the XDG association files — no `xdg-mime`/`gio`
//! shell-out. MIME for a path is taken from a built-in extension map (full
//! shared-mime-info content sniffing is a follow-up).
//!
//! Specs: mime-apps-spec + desktop-entry-spec (freedesktop.org).

use std::path::{Path, PathBuf};

/// A parsed `.desktop` application entry (the fields the manager needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// The desktop file id (basename incl. `.desktop`, e.g. `org.gnome.gedit.desktop`).
    pub id: String,
    /// `Name=` (display name).
    pub name: String,
    /// `Exec=` (raw, with field codes).
    pub exec: String,
    /// `MimeType=` associations declared by the entry.
    pub mime_types: Vec<String>,
}

impl DesktopEntry {
    /// Parse a `.desktop` file's `[Desktop Entry]` group. `None` if it has no
    /// `Exec`/`Name` or isn't a desktop entry.
    #[must_use]
    pub fn parse(path: &Path) -> Option<Self> {
        let body = std::fs::read_to_string(path).ok()?;
        let id = path.file_name()?.to_string_lossy().into_owned();
        let mut in_entry = false;
        let (mut name, mut exec, mut mime) = (None, None, Vec::new());
        for line in body.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some(v) = line.strip_prefix("Name=") {
                name.get_or_insert_with(|| v.to_string());
            } else if let Some(v) = line.strip_prefix("Exec=") {
                exec.get_or_insert_with(|| v.to_string());
            } else if let Some(v) = line.strip_prefix("MimeType=") {
                mime = v
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        }
        Some(Self {
            id,
            name: name?,
            exec: exec?,
            mime_types: mime,
        })
    }

    /// The launch argv for `files`, expanding the desktop `Exec` field codes:
    /// `%f`/`%F` (file path[s]), `%u`/`%U` (same, as given), `%%` → `%`, and the
    /// deprecated/ignored codes (`%i %c %k %d %D %n %N %v %m`) dropped. A bare
    /// `%f`/`%u` with no file is omitted; `%F`/`%U` expand to every file.
    #[must_use]
    pub fn command(&self, files: &[&str]) -> Vec<String> {
        let mut argv = Vec::new();
        for tok in self.exec.split_whitespace() {
            match tok {
                "%f" | "%u" => {
                    if let Some(first) = files.first() {
                        argv.push((*first).to_string());
                    }
                }
                "%F" | "%U" => argv.extend(files.iter().map(|f| (*f).to_string())),
                "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
                other => argv.push(other.replace("%%", "%")),
            }
        }
        argv
    }
}

/// XDG config dirs for the `mimeapps.list` chain: `$XDG_CONFIG_HOME` (or
/// `~/.config`) then `$XDG_CONFIG_DIRS` (default `/etc/xdg`).
#[must_use]
pub fn config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(h));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config"));
    }
    let extra = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".to_string());
    dirs.extend(
        extra
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// XDG data dirs holding `applications/`: `$XDG_DATA_HOME` (or
/// `~/.local/share`) then `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`).
#[must_use]
pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(h));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share"));
    }
    let extra = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    dirs.extend(
        extra
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// The default desktop-file id for `mime`, scanning the `mimeapps.list`
/// `[Default Applications]` group across `config_dirs` in order (first match
/// wins — the user's `~/.config` overrides system defaults).
#[must_use]
pub fn default_app_id(mime: &str, config_dirs: &[PathBuf]) -> Option<String> {
    for dir in config_dirs {
        if let Some(id) = read_default_from(&dir.join("mimeapps.list"), mime) {
            return Some(id);
        }
    }
    None
}

/// Read the `[Default Applications]` value for `mime` from one `mimeapps.list`.
fn read_default_from(path: &Path, mime: &str) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut in_default = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_default = line == "[Default Applications]";
            continue;
        }
        if in_default {
            if let Some((key, val)) = line.split_once('=') {
                if key.trim() == mime {
                    // value may list several ids `;`-separated; take the first.
                    return val
                        .split(';')
                        .map(str::trim)
                        .find(|s| !s.is_empty())
                        .map(str::to_string);
                }
            }
        }
    }
    None
}

/// Locate + parse `applications/<id>` under the `data_dirs` (first found wins).
#[must_use]
pub fn find_entry(app_id: &str, data_dirs: &[PathBuf]) -> Option<DesktopEntry> {
    for dir in data_dirs {
        let path = dir.join("applications").join(app_id);
        if path.is_file() {
            return DesktopEntry::parse(&path);
        }
    }
    None
}

/// Resolve the default [`DesktopEntry`] for `mime` (default id → entry).
#[must_use]
pub fn default_entry(
    mime: &str,
    config_dirs: &[PathBuf],
    data_dirs: &[PathBuf],
) -> Option<DesktopEntry> {
    let id = default_app_id(mime, config_dirs)?;
    find_entry(&id, data_dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mde-files-desktop-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn parses_a_desktop_entry() {
        let dir = scratch("parse");
        let f = dir.join("editor.desktop");
        write(
            &f,
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor %F\nMimeType=text/plain;text/markdown;\n[Desktop Action new]\nName=New\n",
        );
        let e = DesktopEntry::parse(&f).unwrap();
        assert_eq!(e.id, "editor.desktop");
        assert_eq!(e.name, "Editor");
        assert_eq!(e.exec, "editor %F");
        assert_eq!(e.mime_types, vec!["text/plain", "text/markdown"]);
    }

    #[test]
    fn exec_field_codes_expand() {
        let e = DesktopEntry {
            id: "x.desktop".into(),
            name: "X".into(),
            exec: "tool --flag %U %i".into(),
            mime_types: vec![],
        };
        assert_eq!(e.command(&["/a", "/b"]), vec!["tool", "--flag", "/a", "/b"]);
        // %f takes only the first file; %i (icon) is dropped.
        let e2 = DesktopEntry {
            exec: "v %f".into(),
            ..e.clone()
        };
        assert_eq!(e2.command(&["/only", "/ignored"]), vec!["v", "/only"]);
        // no file -> the %f slot is omitted entirely.
        assert_eq!(e2.command(&[]), vec!["v"]);
        // %% literal percent.
        let e3 = DesktopEntry {
            exec: "p 100%%".into(),
            ..e
        };
        assert_eq!(e3.command(&[]), vec!["p", "100%"]);
    }

    #[test]
    fn default_app_resolution_prefers_user_config() {
        let base = scratch("resolve");
        let user_cfg = base.join("config");
        let sys_cfg = base.join("etc-xdg");
        // system default says system-viewer; user override says my-viewer.
        write(
            &sys_cfg.join("mimeapps.list"),
            "[Default Applications]\nimage/png=system-viewer.desktop\n",
        );
        write(
            &user_cfg.join("mimeapps.list"),
            "[Default Applications]\nimage/png=my-viewer.desktop;fallback.desktop;\n",
        );
        let cfg = vec![user_cfg.clone(), sys_cfg.clone()];
        // user config wins, and the first id in the `;`-list is taken.
        assert_eq!(
            default_app_id("image/png", &cfg).as_deref(),
            Some("my-viewer.desktop")
        );
        // a mime nobody claims -> None.
        assert_eq!(default_app_id("application/x-nope", &cfg), None);
    }

    #[test]
    fn default_entry_finds_and_parses_the_handler() {
        let base = scratch("entry");
        let cfg = base.join("config");
        let data = base.join("data");
        write(
            &cfg.join("mimeapps.list"),
            "[Default Applications]\napplication/pdf=reader.desktop\n",
        );
        write(
            &data.join("applications/reader.desktop"),
            "[Desktop Entry]\nName=Reader\nExec=reader %f\nMimeType=application/pdf;\n",
        );
        let entry = default_entry("application/pdf", &[cfg], &[data]).unwrap();
        assert_eq!(entry.name, "Reader");
        assert_eq!(entry.command(&["/doc.pdf"]), vec!["reader", "/doc.pdf"]);
    }
}
