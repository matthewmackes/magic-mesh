//! The multi-server config + saved-token store.
//!
//! Holds N configured Jellyfin servers (each a base URL + an optional saved
//! [`ServerAuth`] = `AccessToken` + `UserId`) and round-trips through a `0600`
//! JSON at the user config dir.
//!
//! # Why tokens, never passwords
//!
//! The username/password flow ([`JellyfinClient::authenticate_by_name`](crate::JellyfinClient::authenticate_by_name))
//! sends the password once and returns an `AccessToken`; only that token is
//! stored. **No plaintext password is ever written** — [`ServerAuth`] has no
//! password field, so it cannot be.
//!
//! # Why not the mesh sealing path
//!
//! The workspace's sealing modules (`mackesd`'s `secrets` / `ca::seal`,
//! `mde-kdc-proto`'s crypto) live inside the mesh control-plane daemon; a
//! desktop media client depending on them would break the mesh/desktop crate
//! boundary (§6). `mde-musicd`'s creds loader is the closest desktop-side
//! precedent, and it stores a plaintext password we deliberately never keep —
//! so per the MEDIA-9 spec this uses a documented `0600` JSON at the user
//! config dir instead.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::AuthenticationResult;

/// The store path relative to the user config dir.
pub const STORE_REL_PATH: &str = "mde/jellyfin/servers.json";

/// A server's saved authentication — the token + user the browse calls use.
///
/// This is the *entire* persisted secret surface: a bearer token and the ids
/// it is bound to. There is intentionally no password field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ServerAuth {
    /// The bearer token sent in the `Authorization` header.
    pub access_token: String,
    /// The authenticated user's GUID (every browse query is scoped to it).
    pub user_id: String,
    /// The user's display name, if known.
    #[serde(default)]
    pub user_name: Option<String>,
    /// The server's GUID, if known.
    #[serde(default)]
    pub server_id: Option<String>,
}

/// One configured Jellyfin server: where it is + how we're signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    /// A stable local id for this entry (the server GUID once known, else a
    /// caller-chosen slug).
    pub id: String,
    /// A human display name for the server.
    pub name: String,
    /// The base URL, e.g. `https://jelly.mesh:8096` (no trailing slash needed).
    pub base_url: String,
    /// The saved auth, once the user has signed in.
    #[serde(default)]
    pub auth: Option<ServerAuth>,
}

impl ServerConfig {
    /// A server entry with no saved auth yet.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            base_url: base_url.into(),
            auth: None,
        }
    }

    /// Whether this entry is signed in (has a saved token).
    #[must_use]
    pub const fn is_authenticated(&self) -> bool {
        self.auth.is_some()
    }
}

/// The persisted set of configured Jellyfin servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ServerStore {
    /// The configured servers.
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
}

/// Why a store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The store file does not exist yet (first run).
    #[error("jellyfin server store not found at {0}")]
    Missing(PathBuf),
    /// The store file could not be read or written.
    #[error("jellyfin server store io error: {0}")]
    Io(String),
    /// The store file was not valid JSON.
    #[error("jellyfin server store parse error: {0}")]
    Parse(String),
}

impl ServerStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Find a server by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ServerConfig> {
        self.servers.iter().find(|s| s.id == id)
    }

    /// Insert `server`, replacing any existing entry with the same id.
    pub fn upsert(&mut self, server: ServerConfig) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.id == server.id) {
            *existing = server;
        } else {
            self.servers.push(server);
        }
    }

    /// Remove the server with `id`. Returns whether one was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.servers.len();
        self.servers.retain(|s| s.id != id);
        self.servers.len() != before
    }

    /// Attach / replace the saved auth on the server with `id`. Returns whether
    /// that server exists.
    pub fn set_auth(&mut self, id: &str, auth: ServerAuth) -> bool {
        if let Some(server) = self.servers.iter_mut().find(|s| s.id == id) {
            server.auth = Some(auth);
            true
        } else {
            false
        }
    }

    /// The default store path: `<config dir>/mde/jellyfin/servers.json`.
    ///
    /// Uses `dirs::config_dir()` (honoring `XDG_CONFIG_HOME`), falling back to
    /// `$HOME/.config` when it cannot be resolved.
    #[must_use]
    pub fn default_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            Path::new(&home).join(".config")
        });
        base.join("mde").join("jellyfin").join("servers.json")
    }

    /// Load the store from `path`, distinguishing a first-run absence
    /// ([`StoreError::Missing`]) from an io / parse failure.
    ///
    /// # Errors
    /// [`StoreError::Missing`] when absent, [`StoreError::Io`] /
    /// [`StoreError::Parse`] otherwise.
    pub fn load_from(path: &Path) -> Result<Self, StoreError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| StoreError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::Missing(path.to_path_buf()))
            }
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    /// Load the store from [`default_path`](Self::default_path).
    ///
    /// # Errors
    /// As [`load_from`](Self::load_from).
    pub fn load() -> Result<Self, StoreError> {
        Self::load_from(&Self::default_path())
    }

    /// Write the store to `path` (creating parent dirs) as pretty JSON with
    /// `0600` (owner-only) permissions — it holds bearer tokens.
    ///
    /// # Errors
    /// [`StoreError::Io`] on a filesystem failure, [`StoreError::Parse`] if
    /// serialization fails.
    pub fn save_to(&self, path: &Path) -> Result<(), StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        let json =
            serde_json::to_string_pretty(self).map_err(|e| StoreError::Parse(e.to_string()))?;
        write_private(path, json.as_bytes()).map_err(|e| StoreError::Io(e.to_string()))
    }

    /// Write the store to [`default_path`](Self::default_path) with `0600`.
    ///
    /// # Errors
    /// As [`save_to`](Self::save_to).
    pub fn save(&self) -> Result<(), StoreError> {
        self.save_to(&Self::default_path())
    }
}

impl AuthenticationResult {
    /// Project a successful login into the persisted [`ServerAuth`] — the token
    /// + the ids it is bound to (no password, ever).
    #[must_use]
    pub fn into_auth(self) -> ServerAuth {
        let name = self.user.name;
        let user_name = if name.is_empty() { None } else { Some(name) };
        ServerAuth {
            access_token: self.access_token,
            user_id: self.user.id,
            user_name,
            server_id: self.server_id,
        }
    }
}

/// Write `bytes` to `path` with owner-only (`0600`) permissions.
///
/// On Unix the file is created `0600` and its mode is re-asserted (covering a
/// pre-existing file with looser perms); elsewhere it is a plain write.
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
    use crate::models::PublicUser;
    use tempfile::tempdir;

    fn sample_store() -> ServerStore {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh:8096"));
        store.upsert(ServerConfig::new("srv-b", "Backup", "https://b.mesh:8096"));
        store.set_auth(
            "srv-a",
            ServerAuth {
                access_token: "TOKEN-A".into(),
                user_id: "user-a".into(),
                user_name: Some("matthew".into()),
                server_id: Some("srv-a".into()),
            },
        );
        store
    }

    #[test]
    fn upsert_replaces_by_id_and_keeps_others() {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("id1", "First", "https://one.mesh"));
        store.upsert(ServerConfig::new("id2", "Second", "https://two.mesh"));
        // Replace id1.
        store.upsert(ServerConfig::new(
            "id1",
            "First-Renamed",
            "https://one.mesh",
        ));
        assert_eq!(store.servers.len(), 2);
        assert_eq!(store.get("id1").expect("id1").name, "First-Renamed");
        assert_eq!(store.get("id2").expect("id2").name, "Second");
    }

    #[test]
    fn remove_and_get() {
        let mut store = sample_store();
        assert!(store.get("srv-a").is_some());
        assert!(store.remove("srv-a"));
        assert!(!store.remove("srv-a")); // already gone
        assert!(store.get("srv-a").is_none());
        assert!(store.get("srv-b").is_some());
    }

    #[test]
    fn set_auth_marks_authenticated() {
        let mut store = sample_store();
        assert!(store.get("srv-a").expect("a").is_authenticated());
        assert!(!store.get("srv-b").expect("b").is_authenticated());
        // Unknown server → false, no panic.
        assert!(!store.set_auth("nope", ServerAuth::default()));
    }

    #[test]
    fn round_trips_through_json() {
        let store = sample_store();
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("sub").join("servers.json"); // parent created
        store.save_to(&path).expect("save");
        let loaded = ServerStore::load_from(&path).expect("load");
        assert_eq!(loaded, store);
        assert_eq!(
            loaded
                .get("srv-a")
                .expect("a")
                .auth
                .as_ref()
                .expect("auth")
                .access_token,
            "TOKEN-A"
        );
    }

    #[test]
    #[cfg(unix)]
    fn saved_file_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("servers.json");
        sample_store().save_to(&path).expect("save");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "token store must be 0600");
    }

    #[test]
    fn serialized_json_never_contains_a_password() {
        let json = serde_json::to_string(&sample_store()).expect("serialize");
        assert!(
            json.contains("access_token"),
            "the token is what we persist"
        );
        // The invariant: no plaintext-password field can appear.
        assert!(!json.to_lowercase().contains("password"));
        assert!(!json.contains("\"Pw\""));
    }

    #[test]
    fn missing_file_is_first_run() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nope.json");
        let err = ServerStore::load_from(&path).expect_err("missing file must error");
        assert!(
            matches!(&err, StoreError::Missing(got) if got == &path),
            "expected Missing({}), got {err:?}",
            path.display()
        );
    }

    #[test]
    fn malformed_file_is_parse_error() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").expect("write");
        assert!(matches!(
            ServerStore::load_from(&path),
            Err(StoreError::Parse(_))
        ));
    }

    #[test]
    fn default_path_ends_with_the_store_rel_path() {
        let p = ServerStore::default_path();
        assert!(
            p.ends_with("mde/jellyfin/servers.json"),
            "got {}",
            p.display()
        );
    }

    #[test]
    fn auth_result_projects_into_server_auth() {
        let result = AuthenticationResult {
            access_token: "T".into(),
            server_id: Some("srv".into()),
            user: PublicUser {
                id: "u1".into(),
                name: "matthew".into(),
                server_id: None,
            },
        };
        let auth = result.into_auth();
        assert_eq!(auth.access_token, "T");
        assert_eq!(auth.user_id, "u1");
        assert_eq!(auth.user_name.as_deref(), Some("matthew"));
        assert_eq!(auth.server_id.as_deref(), Some("srv"));
    }
}
