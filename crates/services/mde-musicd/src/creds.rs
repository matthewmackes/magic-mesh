//! AIR-4 (v6.1) — Airsonic credential loader.
//!
//! Creds live at `~/.local/share/mde/airsonic-creds.json` — under the
//! mesh-shared data dir (Q4: a single shared credential the whole
//! workgroup uses, replicated by `mesh-storage`). The daemon refuses to
//! start without them, pointing the operator at the first-run flow.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Path of the creds file relative to `$HOME`.
pub const CREDS_REL_PATH: &str = ".local/share/mde/airsonic-creds.json";

/// The log line shown when creds are missing (AIR-4 acceptance).
pub const MISSING_HINT: &str =
    "mde-musicd: airsonic creds missing — run `mde-music --first-run` to create";

/// Stored Airsonic credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creds {
    /// Base server URL, e.g. `http://airsonic.anvil.mesh:4040`.
    pub server_url: String,
    pub username: String,
    pub password: String,
}

/// Why loading creds failed.
#[derive(Debug)]
pub enum CredsError {
    /// The file doesn't exist — first run hasn't happened.
    Missing(PathBuf),
    /// The file exists but couldn't be read.
    Io(std::io::Error),
    /// The file exists but isn't valid creds JSON.
    Parse(serde_json::Error),
}

impl std::fmt::Display for CredsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "{MISSING_HINT} (looked at {})", p.display()),
            Self::Io(e) => write!(f, "mde-musicd: reading airsonic creds: {e}"),
            Self::Parse(e) => write!(f, "mde-musicd: airsonic creds malformed: {e}"),
        }
    }
}

impl std::error::Error for CredsError {}

/// Default creds path: `$HOME/.local/share/mde/airsonic-creds.json`
/// (falls back to `/root` when `$HOME` is unset).
#[must_use]
pub fn default_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(CREDS_REL_PATH)
}

/// Load creds from `path`, distinguishing missing (first-run) from
/// malformed.
///
/// # Errors
/// [`CredsError::Missing`] when absent, `Io`/`Parse` otherwise.
pub fn load_from(path: &Path) -> Result<Creds, CredsError> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(CredsError::Parse),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(CredsError::Missing(path.to_path_buf()))
        }
        Err(e) => Err(CredsError::Io(e)),
    }
}

/// Load creds from the [`default_path`].
///
/// # Errors
/// As [`load_from`].
pub fn load() -> Result<Creds, CredsError> {
    load_from(&default_path())
}

/// Whether a candidate server URL + username are well-formed enough to
/// save: a non-empty `http(s)://…` URL + a non-empty username. (The
/// password may legitimately be empty on an open server.)
#[must_use]
pub fn is_valid(server_url: &str, username: &str) -> bool {
    let url = server_url.trim();
    !username.trim().is_empty()
        && (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() > "https://".len()
}

/// Write `creds` to `path` (creating the parent dir), pretty-printed.
///
/// # Errors
/// IO / serialization failures.
pub fn save_to(path: &Path, creds: &Creds) -> Result<(), CredsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CredsError::Io)?;
    }
    let json = serde_json::to_string_pretty(creds).map_err(CredsError::Parse)?;
    std::fs::write(path, json).map_err(CredsError::Io)
}

/// Write `creds` to the [`default_path`].
///
/// # Errors
/// As [`save_to`].
pub fn save(creds: &Creds) -> Result<(), CredsError> {
    save_to(&default_path(), creds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn missing_file_is_first_run_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("nope.json");
        match load_from(&p) {
            Err(CredsError::Missing(got)) => assert_eq!(got, p),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn missing_message_carries_the_hint() {
        let dir = tempdir().unwrap();
        let err = load_from(&dir.path().join("nope.json")).unwrap_err();
        assert!(err.to_string().contains("airsonic creds missing"));
        assert!(err.to_string().contains("mde-music --first-run"));
    }

    #[test]
    fn valid_file_round_trips() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("airsonic-creds.json");
        let creds = Creds {
            server_url: "http://airsonic.anvil.mesh:4040".into(),
            username: "alice".into(),
            password: "sesame".into(),
        };
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(serde_json::to_string_pretty(&creds).unwrap().as_bytes())
            .unwrap();
        assert_eq!(load_from(&p).unwrap(), creds);
    }

    #[test]
    fn malformed_file_is_parse_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bad.json");
        std::fs::write(&p, "{not json").unwrap();
        assert!(matches!(load_from(&p), Err(CredsError::Parse(_))));
    }

    #[test]
    fn default_path_is_under_mesh_data_dir() {
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(
            default_path(),
            Path::new("/home/tester/.local/share/mde/airsonic-creds.json")
        );
    }

    #[test]
    fn is_valid_requires_http_url_and_username() {
        assert!(is_valid("http://airsonic.mesh:4040", "alice"));
        assert!(is_valid("https://music.example.com", "bob"));
        // Empty password is allowed (open server).
        assert!(is_valid("http://h:4040", "u"));
        // Rejections.
        assert!(!is_valid("airsonic.mesh:4040", "alice")); // no scheme
        assert!(!is_valid("http://h", "")); // no username
        assert!(!is_valid("https://", "alice")); // scheme only
        assert!(!is_valid("", "alice"));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("sub").join("airsonic-creds.json"); // parent created
        let creds = Creds {
            server_url: "http://airsonic.mesh:4040".into(),
            username: "alice".into(),
            password: "sesame".into(),
        };
        save_to(&p, &creds).unwrap();
        assert_eq!(load_from(&p).unwrap(), creds);
    }
}
