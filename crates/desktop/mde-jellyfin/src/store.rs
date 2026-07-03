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

/// One configured Jellyfin server: where it is + who is signed in.
///
/// A server can hold **N user profiles** ([`profiles`](Self::profiles)) — each its
/// own [`ServerAuth`] (`AccessToken` + `UserId`), keyed by the user's GUID — with
/// one [`active_profile`](Self::active_profile) at a time. The [`auth`](Self::auth)
/// field mirrors the active profile so the whole browse / play / report path keeps
/// reading "the signed-in credentials" without knowing profiles exist (MEDIA-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    /// A stable local id for this entry (the server GUID once known, else a
    /// caller-chosen slug).
    pub id: String,
    /// A human display name for the server.
    pub name: String,
    /// The base URL, e.g. `https://jelly.mesh:8096` (no trailing slash needed).
    pub base_url: String,
    /// The active profile's saved auth — a **mirror** of the
    /// [`active_profile`](Self::active_profile) entry in [`profiles`](Self::profiles),
    /// kept so the existing browse/play path reads one field. Switching a profile
    /// re-points this.
    #[serde(default)]
    pub auth: Option<ServerAuth>,
    /// The configured user profiles (each its own token + user), keyed by
    /// [`ServerAuth::user_id`]. Empty until the first sign-in.
    #[serde(default)]
    pub profiles: Vec<ServerAuth>,
    /// The `user_id` of the active profile, if one is selected.
    #[serde(default)]
    pub active_profile: Option<String>,
}

impl ServerConfig {
    /// A server entry with no saved auth / profiles yet.
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
            profiles: Vec::new(),
            active_profile: None,
        }
    }

    /// Whether this entry has any signed-in profile (an active token).
    #[must_use]
    pub const fn is_authenticated(&self) -> bool {
        self.auth.is_some()
    }

    /// The configured user profiles (each its own token + user).
    #[must_use]
    pub fn profiles(&self) -> &[ServerAuth] {
        &self.profiles
    }

    /// The active profile's auth, if one is selected (same as [`auth`](Self::auth)).
    #[must_use]
    pub const fn active_auth(&self) -> Option<&ServerAuth> {
        self.auth.as_ref()
    }

    /// Add or replace a user profile (keyed by its [`user_id`](ServerAuth::user_id)).
    ///
    /// The first profile added becomes the active one; a later add keeps the
    /// current selection but refreshes that profile's token if it matches. The
    /// mirrored [`auth`](Self::auth) is re-synced either way.
    pub fn add_profile(&mut self, auth: ServerAuth) {
        let user_id = auth.user_id.clone();
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.user_id == user_id) {
            *existing = auth;
        } else {
            self.profiles.push(auth);
        }
        if self.active_profile.is_none() {
            self.active_profile = Some(user_id);
        }
        self.sync_active_auth();
    }

    /// Switch the active profile to the one with `user_id`. Returns whether such a
    /// profile exists (a no-op + `false` for an unknown user).
    pub fn switch_profile(&mut self, user_id: &str) -> bool {
        if self.profiles.iter().any(|p| p.user_id == user_id) {
            self.active_profile = Some(user_id.to_owned());
            self.sync_active_auth();
            true
        } else {
            false
        }
    }

    /// Remove the profile with `user_id`. Returns whether one was removed; if it
    /// was the active profile, the first remaining profile (or none) takes over.
    pub fn remove_profile(&mut self, user_id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.user_id != user_id);
        let removed = self.profiles.len() != before;
        if removed {
            if self.active_profile.as_deref() == Some(user_id) {
                self.active_profile = self.profiles.first().map(|p| p.user_id.clone());
            }
            self.sync_active_auth();
        }
        removed
    }

    /// Re-point the mirrored [`auth`](Self::auth) at the active profile (or `None`).
    fn sync_active_auth(&mut self) {
        self.auth = self
            .active_profile
            .as_ref()
            .and_then(|id| self.profiles.iter().find(|p| &p.user_id == id))
            .cloned();
    }

    /// Reconcile the profile fields with a possibly-older on-disk shape: seed a
    /// profile from a lone [`auth`](Self::auth) (a pre-MEDIA-11 store), or adopt the
    /// first profile as active when none is selected, then re-sync the mirror.
    fn normalize(&mut self) {
        if self.profiles.is_empty() {
            if let Some(auth) = self.auth.clone() {
                self.active_profile = Some(auth.user_id.clone());
                self.profiles.push(auth);
            }
        } else if self.active_profile.is_none() {
            self.active_profile = self.profiles.first().map(|p| p.user_id.clone());
        }
        self.sync_active_auth();
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

    /// Attach / replace the saved auth on the server with `id` — a sign-in. Adds
    /// (or refreshes) the matching user profile and makes it active. Returns
    /// whether that server exists.
    pub fn set_auth(&mut self, id: &str, auth: ServerAuth) -> bool {
        self.add_profile(id, auth)
    }

    /// Add / refresh a user profile on the server with `id` (MEDIA-11 multi-user).
    /// Returns whether that server exists.
    pub fn add_profile(&mut self, id: &str, auth: ServerAuth) -> bool {
        self.servers
            .iter_mut()
            .find(|s| s.id == id)
            .map(|server| server.add_profile(auth))
            .is_some()
    }

    /// Switch the active profile on the server with `id` to `user_id`. Returns
    /// whether both the server and that profile exist.
    pub fn switch_profile(&mut self, id: &str, user_id: &str) -> bool {
        self.servers
            .iter_mut()
            .find(|s| s.id == id)
            .is_some_and(|server| server.switch_profile(user_id))
    }

    /// Remove a user profile from the server with `id`. Returns whether one was
    /// removed.
    pub fn remove_profile(&mut self, id: &str, user_id: &str) -> bool {
        self.servers
            .iter_mut()
            .find(|s| s.id == id)
            .is_some_and(|server| server.remove_profile(user_id))
    }

    /// The default store path: `<config dir>/mde/jellyfin/servers.json`.
    ///
    /// Uses `dirs::config_dir()` (honoring `XDG_CONFIG_HOME`), falling back to
    /// `$HOME/.config` when it cannot be resolved.
    #[must_use]
    pub fn default_path() -> PathBuf {
        config_base()
            .join("mde")
            .join("jellyfin")
            .join("servers.json")
    }

    /// Load the store from `path`, distinguishing a first-run absence
    /// ([`StoreError::Missing`]) from an io / parse failure. The loaded store is
    /// [`normalize`](ServerConfig::normalize)d, so a pre-MEDIA-11 shape (a lone
    /// `auth`, no profiles) is migrated into a single active profile.
    ///
    /// # Errors
    /// [`StoreError::Missing`] when absent, [`StoreError::Io`] /
    /// [`StoreError::Parse`] otherwise.
    pub fn load_from(path: &Path) -> Result<Self, StoreError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut store: Self =
                    serde_json::from_str(&text).map_err(|e| StoreError::Parse(e.to_string()))?;
                for server in &mut store.servers {
                    server.normalize();
                }
                Ok(store)
            }
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

/// The user config base dir: `dirs::config_dir()` (honoring `XDG_CONFIG_HOME`),
/// falling back to `$HOME/.config`. Shared by the server store + the offline
/// cache ([`crate::cache`]) so both root under the same `mde/jellyfin/` tree.
pub(crate) fn config_base() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        Path::new(&home).join(".config")
    })
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

    fn profile(user_id: &str, name: &str, token: &str) -> ServerAuth {
        ServerAuth {
            access_token: token.into(),
            user_id: user_id.into(),
            user_name: Some(name.into()),
            server_id: Some("srv-a".into()),
        }
    }

    #[test]
    fn set_auth_seeds_a_first_active_profile() {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh"));
        assert!(store.set_auth("srv-a", profile("user-a", "matthew", "TOKEN-A")));
        let server = store.get("srv-a").expect("srv-a");
        // The lone sign-in became the sole, active profile, and mirrored to `auth`.
        assert_eq!(server.profiles().len(), 1);
        assert_eq!(server.active_profile.as_deref(), Some("user-a"));
        assert_eq!(
            server.active_auth().expect("active").access_token,
            "TOKEN-A"
        );
    }

    #[test]
    fn multiple_profiles_switch_with_per_profile_token_isolation() {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh"));
        store.add_profile("srv-a", profile("user-a", "matthew", "TOKEN-A"));
        store.add_profile("srv-a", profile("user-b", "guest", "TOKEN-B"));

        let server = store.get("srv-a").expect("srv-a");
        assert_eq!(server.profiles().len(), 2);
        // The first-added profile is active; its token mirrors to `auth`.
        assert_eq!(server.active_profile.as_deref(), Some("user-a"));
        assert_eq!(server.active_auth().expect("a").access_token, "TOKEN-A");

        // Switching flips the active token — each profile keeps its own.
        assert!(store.switch_profile("srv-a", "user-b"));
        let server = store.get("srv-a").expect("srv-a");
        assert_eq!(server.active_auth().expect("b").access_token, "TOKEN-B");
        assert_eq!(server.active_auth().expect("b").user_id, "user-b");
        // Both tokens still live in the store, unmixed.
        let tokens: Vec<&str> = server
            .profiles()
            .iter()
            .map(|p| p.access_token.as_str())
            .collect();
        assert!(tokens.contains(&"TOKEN-A") && tokens.contains(&"TOKEN-B"));

        // Switching to an unknown user is refused, leaving the selection intact.
        assert!(!store.switch_profile("srv-a", "nobody"));
        assert_eq!(
            store
                .get("srv-a")
                .expect("srv-a")
                .active_auth()
                .expect("b")
                .access_token,
            "TOKEN-B"
        );
    }

    #[test]
    fn re_adding_a_profile_refreshes_its_token_without_reordering() {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh"));
        store.add_profile("srv-a", profile("user-a", "matthew", "OLD"));
        store.add_profile("srv-a", profile("user-b", "guest", "TOKEN-B"));
        store.switch_profile("srv-a", "user-b");
        // Refresh user-a's token; the active profile (user-b) is unchanged.
        store.add_profile("srv-a", profile("user-a", "matthew", "NEW"));
        let server = store.get("srv-a").expect("srv-a");
        assert_eq!(server.profiles().len(), 2);
        assert_eq!(server.active_profile.as_deref(), Some("user-b"));
        let a = server
            .profiles()
            .iter()
            .find(|p| p.user_id == "user-a")
            .expect("user-a");
        assert_eq!(a.access_token, "NEW");
    }

    #[test]
    fn removing_the_active_profile_promotes_another() {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh"));
        store.add_profile("srv-a", profile("user-a", "matthew", "TOKEN-A"));
        store.add_profile("srv-a", profile("user-b", "guest", "TOKEN-B"));
        // user-a is active; removing it promotes the remaining profile.
        assert!(store.remove_profile("srv-a", "user-a"));
        let server = store.get("srv-a").expect("srv-a");
        assert_eq!(server.profiles().len(), 1);
        assert_eq!(server.active_profile.as_deref(), Some("user-b"));
        assert_eq!(server.active_auth().expect("b").access_token, "TOKEN-B");
        // Removing the last profile leaves the server signed-out.
        assert!(store.remove_profile("srv-a", "user-b"));
        let server = store.get("srv-a").expect("srv-a");
        assert!(server.profiles().is_empty());
        assert!(!server.is_authenticated());
        assert!(server.active_profile.is_none());
    }

    #[test]
    fn legacy_store_with_lone_auth_migrates_to_a_profile_on_load() {
        // A pre-MEDIA-11 store: `auth` set, no `profiles` / `active_profile`.
        let legacy = r#"{"servers":[{"id":"srv-a","name":"Anvil",
            "base_url":"https://a.mesh","auth":{"access_token":"T","user_id":"u1",
            "user_name":"matthew","server_id":"srv-a"}}]}"#;
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("servers.json");
        std::fs::write(&path, legacy).expect("write");
        let store = ServerStore::load_from(&path).expect("load");
        let server = store.get("srv-a").expect("srv-a");
        // The lone auth is migrated into a single active profile.
        assert_eq!(server.profiles().len(), 1);
        assert_eq!(server.active_profile.as_deref(), Some("u1"));
        assert_eq!(server.active_auth().expect("auth").access_token, "T");
    }

    #[test]
    fn profiles_round_trip_through_json() {
        let mut store = ServerStore::new();
        store.upsert(ServerConfig::new("srv-a", "Anvil", "https://a.mesh"));
        store.add_profile("srv-a", profile("user-a", "matthew", "TOKEN-A"));
        store.add_profile("srv-a", profile("user-b", "guest", "TOKEN-B"));
        store.switch_profile("srv-a", "user-b");
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("servers.json");
        store.save_to(&path).expect("save");
        let loaded = ServerStore::load_from(&path).expect("load");
        assert_eq!(loaded, store);
        assert_eq!(
            loaded
                .get("srv-a")
                .expect("srv-a")
                .active_auth()
                .expect("active")
                .access_token,
            "TOKEN-B"
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
