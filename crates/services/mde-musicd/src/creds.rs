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

/// Optional absolute path override used by packaged/system sessions.
///
/// The DRM shell is intentionally root-owned, while the interactive music
/// daemon runs as the logged-in seat user.  Keeping the override explicit
/// avoids baking a particular username into the client.
pub const CREDS_PATH_ENV: &str = "MDE_AIRSONIC_CREDS";

/// Optional bounded list of additional read sources.  The legacy primary
/// credential remains the first source and remains the single writer for
/// playlist/transport mutations.
pub const SOURCES_REL_PATH: &str = ".local/share/mde/airsonic-sources.json";
/// Environment override for the optional source-list path.
pub const SOURCES_PATH_ENV: &str = "MDE_AIRSONIC_SOURCES";
/// Maximum number of primary plus additional source credentials admitted.
pub const MAX_CONFIGURED_SOURCES: usize = 4;
const SOURCES_SCHEMA_VERSION: u16 = 1;

/// The log line shown when creds are missing (AIR-4 acceptance).
pub const MISSING_HINT: &str =
    "mde-musicd: airsonic creds missing — run `mde-music --first-run` to create";

/// Stored Airsonic credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Creds {
    /// Base server URL, e.g. `http://airsonic.anvil.mesh:4040`.
    pub server_url: String,
    /// Subsonic username (the `u=` auth param).
    pub username: String,
    /// Plaintext password — hashed with the per-request salt into a Subsonic token at call time.
    pub password: String,
}

/// Optional multi-source credential envelope.  Passwords stay in the
/// operator-owned 0600 file and are never copied into the catalog/read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcesFile {
    schema_version: u16,
    sources: Vec<Creds>,
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
    /// The file parsed but cannot anchor a real Subsonic/AirSonic session.
    Invalid(String),
}

impl std::fmt::Display for CredsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "{MISSING_HINT} (looked at {})", p.display()),
            Self::Io(e) => write!(f, "mde-musicd: reading airsonic creds: {e}"),
            Self::Parse(e) => write!(f, "mde-musicd: airsonic creds malformed: {e}"),
            Self::Invalid(e) => write!(f, "mde-musicd: airsonic creds invalid: {e}"),
        }
    }
}

impl std::error::Error for CredsError {}

/// Default creds path: `$HOME/.local/share/mde/airsonic-creds.json`.
///
/// A system-owned DRM shell has `HOME=/root`, but the credentials belong to
/// the active graphical seat.  When `XDG_RUNTIME_DIR=/run/user/<uid>` points
/// at a non-root seat, resolve that uid through `/etc/passwd` and use its home
/// directory.  `MDE_AIRSONIC_CREDS` always wins and is intended for unusual
/// packaged/session layouts.
#[must_use]
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os(CREDS_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return path;
    }

    if let Some(home) = active_seat_home() {
        return home.join(CREDS_REL_PATH);
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"));
    home.join(CREDS_REL_PATH)
}

/// Find the home directory of the active non-root graphical seat when this
/// process is running with the root shell's environment.
fn active_seat_home() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if home
        .as_deref()
        .is_some_and(|path| path != Path::new("/root"))
    {
        return None;
    }

    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let uid = runtime.file_name()?.to_str()?.parse::<u32>().ok()?;
    if uid == 0 {
        return None;
    }
    passwd_home(uid)
}

/// Resolve a uid's home from the local passwd database.  This deliberately
/// avoids a user/database dependency in the desktop credential path.
fn passwd_home(uid: u32) -> Option<PathBuf> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd_home_from(uid, &passwd)
}

/// Resolve a uid's home from passwd text.  The fifth field is the optional
/// GECOS/comment field; the home directory is the sixth field.
fn passwd_home_from(uid: u32, passwd: &str) -> Option<PathBuf> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _password = fields.next()?;
        let entry_uid = fields.next()?.parse::<u32>().ok()?;
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        (entry_uid == uid && !home.is_empty()).then(|| PathBuf::from(home))
    })
}

/// Load creds from `path`, distinguishing missing (first-run) from
/// malformed.
///
/// # Errors
/// [`CredsError::Missing`] when absent, `Io`/`Parse` otherwise.
pub fn load_from(path: &Path) -> Result<Creds, CredsError> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let creds: Creds = serde_json::from_str(&s).map_err(CredsError::Parse)?;
            validate(&creds)?;
            Ok(creds)
        }
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

/// Path of the optional additional-source credential file.
#[must_use]
pub fn sources_path() -> PathBuf {
    if let Some(path) = std::env::var_os(SOURCES_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return path;
    }
    default_path().with_file_name("airsonic-sources.json")
}

/// Load the legacy primary source followed by the optional bounded source
/// list.  A missing primary is tolerated only when the optional file contains
/// at least one source, which keeps first-run behavior intact while allowing a
/// source-only deployment.  Duplicate URL/user pairs are admitted once.
pub fn load_all() -> Result<Vec<Creds>, CredsError> {
    let primary = load();
    let extra = match std::fs::read_to_string(sources_path()) {
        Ok(raw) => {
            let file: SourcesFile = serde_json::from_str(&raw).map_err(CredsError::Parse)?;
            if file.schema_version != SOURCES_SCHEMA_VERSION
                || file.sources.len() > MAX_CONFIGURED_SOURCES
            {
                return Err(CredsError::Invalid(
                    "airsonic source list has an unsupported schema or bound violation".into(),
                ));
            }
            for source in &file.sources {
                validate(source)?;
            }
            file.sources
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(CredsError::Io(error)),
    };

    let mut sources = Vec::with_capacity(MAX_CONFIGURED_SOURCES);
    if let Ok(source) = primary.as_ref() {
        sources.push(source.clone());
    }
    for source in extra {
        if sources.iter().any(|existing| {
            existing.server_url == source.server_url && existing.username == source.username
        }) {
            continue;
        }
        if sources.len() == MAX_CONFIGURED_SOURCES {
            break;
        }
        sources.push(source);
    }
    if sources.is_empty() {
        return Err(primary.expect_err("primary load result must exist"));
    }
    Ok(sources)
}

/// Whether a candidate server URL + username are well-formed enough to
/// save: a non-empty `http(s)://…` URL + a non-empty username. (The
/// password may legitimately be empty on an open server.)
#[must_use]
pub fn is_valid(server_url: &str, username: &str) -> bool {
    let url = server_url.trim();
    !username.trim().is_empty() && valid_http_base_url(url)
}

/// Minimal validation for a configured Subsonic base URL. Pathful gateway proxy
/// bases are valid; credential/userinfo, query strings, fragments, whitespace,
/// and empty authorities are not.
fn valid_http_base_url(url: &str) -> bool {
    if url.is_empty() || url.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return false;
    }
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    !authority.is_empty() && !authority.starts_with(':')
}

/// Validate parsed credentials before callers build a long-lived client/session.
/// This keeps a malformed or stale materialized file from silently becoming an
/// unusable Subsonic session anchor.
fn validate(creds: &Creds) -> Result<(), CredsError> {
    if is_valid(&creds.server_url, &creds.username) {
        Ok(())
    } else {
        Err(CredsError::Invalid(
            "server_url must be an http(s) URL and username must be non-empty".to_string(),
        ))
    }
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
    write_private(path, json.as_bytes()).map_err(CredsError::Io)
}

/// Write `creds` to the [`default_path`].
///
/// # Errors
/// As [`save_to`].
pub fn save(creds: &Creds) -> Result<(), CredsError> {
    save_to(&default_path(), creds)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
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
    fn unknown_fields_are_rejected_before_session_build() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("extra.json");
        std::fs::write(
            &p,
            r#"{"server_url":"http://gateway.mesh:4040/mde/airsonic/src","username":"alice","password":"sesame","demo":true}"#,
        )
        .unwrap();
        assert!(matches!(load_from(&p), Err(CredsError::Parse(_))));
    }

    #[test]
    fn invalid_session_anchor_is_rejected() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("invalid.json");
        std::fs::write(
            &p,
            r#"{"server_url":"airsonic.mesh:4040","username":"alice","password":"sesame"}"#,
        )
        .unwrap();
        assert!(matches!(load_from(&p), Err(CredsError::Invalid(_))));
        std::fs::write(
            &p,
            r#"{"server_url":"http://gateway.mesh:4040/mde/airsonic/src","username":"  ","password":"sesame"}"#,
        )
        .unwrap();
        assert!(matches!(load_from(&p), Err(CredsError::Invalid(_))));
    }

    #[test]
    fn default_path_is_under_mesh_data_dir() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        std::env::remove_var(CREDS_PATH_ENV);
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(
            default_path(),
            Path::new("/home/tester/.local/share/mde/airsonic-creds.json")
        );
    }

    #[test]
    fn explicit_path_override_wins() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        std::env::set_var(CREDS_PATH_ENV, "/run/mde/airsonic.json");
        assert_eq!(default_path(), Path::new("/run/mde/airsonic.json"));
        std::env::remove_var(CREDS_PATH_ENV);
    }

    #[test]
    fn passwd_parser_uses_home_not_gecos() {
        let passwd = "mm:x:1000:1000:Seat User:/home/mm:/bin/bash\n";
        assert_eq!(
            passwd_home_from(1000, passwd),
            Some(PathBuf::from("/home/mm"))
        );
    }

    #[test]
    fn is_valid_requires_http_url_and_username() {
        assert!(is_valid("http://airsonic.mesh:4040", "alice"));
        assert!(is_valid("https://music.example.com", "bob"));
        assert!(is_valid(
            "http://gateway.mesh:4040/mde/airsonic/source",
            "alice"
        ));
        // Empty password is allowed (open server).
        assert!(is_valid("http://h:4040", "u"));
        // Rejections.
        assert!(!is_valid("airsonic.mesh:4040", "alice")); // no scheme
        assert!(!is_valid("http://alice@airsonic.mesh:4040", "alice")); // userinfo
        assert!(!is_valid("http://air sonic.mesh:4040", "alice")); // whitespace
        assert!(!is_valid("http://airsonic.mesh:4040?token=x", "alice")); // query
        assert!(!is_valid("http://airsonic.mesh:4040#frag", "alice")); // fragment
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

    #[test]
    fn load_all_keeps_primary_first_and_bounds_duplicates() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let dir = tempdir().unwrap();
        let primary_path = dir.path().join("primary.json");
        let sources_path = dir.path().join("sources.json");
        let primary = Creds {
            server_url: "http://one.test".into(),
            username: "alice".into(),
            password: "one".into(),
        };
        let second = Creds {
            server_url: "http://two.test".into(),
            username: "bob".into(),
            password: "two".into(),
        };
        save_to(&primary_path, &primary).unwrap();
        std::fs::write(
            &sources_path,
            serde_json::to_string(&SourcesFile {
                schema_version: SOURCES_SCHEMA_VERSION,
                sources: vec![primary.clone(), second.clone(), second.clone()],
            })
            .unwrap(),
        )
        .unwrap();
        std::env::set_var(CREDS_PATH_ENV, &primary_path);
        std::env::set_var(SOURCES_PATH_ENV, &sources_path);

        assert_eq!(load_all().unwrap(), vec![primary, second]);

        std::env::remove_var(CREDS_PATH_ENV);
        std::env::remove_var(SOURCES_PATH_ENV);
    }
}
