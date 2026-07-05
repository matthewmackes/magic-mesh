//! IAC-1 — the `clouds.yaml` loader: the openstacksdk-standard auth config the
//! client authenticates with.
//!
//! Design Q20 (`docs/design/iac-workspace.md`): auth is **`clouds.yaml` on the
//! node** — the openstacksdk standard file (`~/.config/openstack/clouds.yaml`,
//! or `OS_CLIENT_CONFIG_FILE`). A **single default context** (Q19 — the mesh's
//! cloud + the operator's project/region, no switcher) is selected. The
//! password lives in the file, so it is **never** placed on a command line
//! (`no password on argv`) — and this type's [`Debug`] redacts it so it can't
//! leak into a log.
//!
//! [`parse_clouds_yaml`] is pure (fixture-tested); [`load_default`] adds the I/O
//! (resolve the standard path, read, select the context).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::ClientError;

// re-export the shared interface enum so callers name one type.
pub use mackes_mesh_types::openstack::EndpointInterface;

/// The resolved single-context auth config — everything the client needs to mint
/// a Keystone token and pick an endpoint interface.
#[derive(Clone, PartialEq, Eq)]
pub struct CloudConfig {
    /// Which `clouds:` entry this came from (the context name).
    pub cloud: String,
    /// The Keystone auth URL (`http://keystone.mesh:5000/v3`).
    pub auth_url: String,
    /// The operator username.
    pub username: String,
    /// The operator password — **never** logged (redacted in [`Debug`]) and
    /// **never** placed on argv (it rides the auth-request JSON body).
    pub password: String,
    /// The scoped project name.
    pub project_name: Option<String>,
    /// The project's domain (defaults to `Default`).
    pub project_domain: String,
    /// The user's domain (defaults to `Default`).
    pub user_domain: String,
    /// The region (Q19 — the operator's single region), when set.
    pub region_name: Option<String>,
    /// Which catalog interface the client reaches services on (default
    /// `public`).
    pub interface: EndpointInterface,
}

impl std::fmt::Debug for CloudConfig {
    /// Redacts the password so a config dump can never leak the credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudConfig")
            .field("cloud", &self.cloud)
            .field("auth_url", &self.auth_url)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("project_name", &self.project_name)
            .field("project_domain", &self.project_domain)
            .field("user_domain", &self.user_domain)
            .field("region_name", &self.region_name)
            .field("interface", &self.interface)
            .finish()
    }
}

// ─────────────────────────── the clouds.yaml schema ───────────────────────────

#[derive(Debug, Deserialize)]
struct CloudsFile {
    #[serde(default)]
    clouds: std::collections::BTreeMap<String, CloudEntry>,
}

#[derive(Debug, Deserialize)]
struct CloudEntry {
    #[serde(default)]
    auth: Option<AuthBlock>,
    #[serde(default)]
    region_name: Option<String>,
    #[serde(default)]
    interface: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthBlock {
    #[serde(default)]
    auth_url: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    project_name: Option<String>,
    #[serde(default)]
    project_domain_name: Option<String>,
    #[serde(default)]
    user_domain_name: Option<String>,
}

/// Parse a `clouds.yaml` body and select the single default context.
///
/// Context selection follows the single-context design (Q19):
/// - if `wanted` (e.g. from `$OS_CLOUD`) names a cloud, that one is used;
/// - else if the file holds exactly **one** cloud, it is used;
/// - else if a cloud named `default` exists, it is used;
/// - else the file is ambiguous (several clouds, none named `default`, no
///   `$OS_CLOUD`) — a typed error, never a silent pick.
///
/// A selected context missing its `auth_url`/`username`/`password` is a typed
/// error (the config is incomplete — honest, never a fabricated default).
///
/// # Errors
/// [`ClientError::Config`] on a YAML parse failure, an empty/ambiguous
/// `clouds:` map, an unknown `wanted` cloud, or a context missing a required
/// auth field.
pub fn parse_clouds_yaml(yaml: &str, wanted: Option<&str>) -> Result<CloudConfig, ClientError> {
    let file: CloudsFile =
        serde_yaml::from_str(yaml).map_err(|e| ClientError::Config(format!("clouds.yaml: {e}")))?;
    if file.clouds.is_empty() {
        return Err(ClientError::Config(
            "clouds.yaml has no `clouds:` entries".to_string(),
        ));
    }

    let name: String = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => {
            if !file.clouds.contains_key(w) {
                return Err(ClientError::Config(format!(
                    "clouds.yaml has no cloud named `{w}` (OS_CLOUD)"
                )));
            }
            w.to_string()
        }
        None if file.clouds.len() == 1 => file
            .clouds
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| ClientError::Config("clouds.yaml has no entries".to_string()))?,
        None if file.clouds.contains_key("default") => "default".to_string(),
        None => {
            let mut names: Vec<&str> = file.clouds.keys().map(String::as_str).collect();
            names.sort_unstable();
            return Err(ClientError::Config(format!(
                "clouds.yaml is ambiguous — {} clouds ({}) and none named `default`; set OS_CLOUD \
                 to pick the single default context",
                names.len(),
                names.join(", ")
            )));
        }
    };

    let Some(entry) = file.clouds.get(&name) else {
        return Err(ClientError::Config(format!(
            "cloud `{name}` vanished from the clouds map"
        )));
    };
    let auth = entry
        .auth
        .as_ref()
        .ok_or_else(|| ClientError::Config(format!("cloud `{name}` has no `auth:` block")))?;

    let require = |field: &Option<String>, what: &str| -> Result<String, ClientError> {
        field
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ClientError::Config(format!("cloud `{name}` auth is missing `{what}`")))
    };

    let interface = entry
        .interface
        .as_deref()
        .and_then(EndpointInterface::parse)
        .unwrap_or(EndpointInterface::Public);

    Ok(CloudConfig {
        cloud: name.clone(),
        auth_url: require(&auth.auth_url, "auth_url")?,
        username: require(&auth.username, "username")?,
        password: require(&auth.password, "password")?,
        project_name: auth
            .project_name
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        project_domain: auth
            .project_domain_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Default".to_string()),
        user_domain: auth
            .user_domain_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Default".to_string()),
        region_name: entry
            .region_name
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        interface,
    })
}

/// The candidate `clouds.yaml` locations, in openstacksdk precedence order.
///
/// `$OS_CLIENT_CONFIG_FILE`, then `~/.config/openstack/clouds.yaml`, then
/// `/etc/openstack/clouds.yaml`. The first that exists wins.
#[must_use]
pub fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(explicit) = std::env::var("OS_CLIENT_CONFIG_FILE") {
        if !explicit.trim().is_empty() {
            paths.push(PathBuf::from(explicit));
        }
    }
    if let Some(cfg) = dirs::config_dir() {
        paths.push(cfg.join("openstack").join("clouds.yaml"));
    }
    paths.push(PathBuf::from("/etc/openstack/clouds.yaml"));
    paths
}

/// Load + select the single default context from the standard `clouds.yaml`
/// location.
///
/// Resolves the first existing [`candidate_paths`] entry, reads it, and selects
/// the context (honoring `$OS_CLOUD`). A **missing** file is
/// [`ClientError::Unconfigured`] — an honest "no cloud configured on this node"
/// the caller surfaces as a gate, distinct from a malformed file
/// ([`ClientError::Config`]).
///
/// # Errors
/// [`ClientError::Unconfigured`] when no `clouds.yaml` exists; [`ClientError::Config`]
/// on a read/parse/selection failure.
pub fn load_default() -> Result<CloudConfig, ClientError> {
    let candidates = candidate_paths();
    let Some(path) = candidates.iter().find(|p| p.exists()) else {
        return Err(ClientError::Unconfigured(format!(
            "no clouds.yaml on this node (looked in {}) — configure the OpenStack context (Q20) to \
             use the IaC workspace",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    load_from(path)
}

/// Load + select from a specific `clouds.yaml` path (the seam `load_default`
/// resolves the path for; tests point it at a fixture).
///
/// # Errors
/// [`ClientError::Config`] on a read failure or a parse/selection failure.
pub fn load_from(path: &Path) -> Result<CloudConfig, ClientError> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| ClientError::Config(format!("reading {}: {e}", path.display())))?;
    let wanted = std::env::var("OS_CLOUD").ok();
    parse_clouds_yaml(&body, wanted.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_CLOUD: &str = r"
clouds:
  mesh:
    auth:
      auth_url: http://keystone.mesh:5000/v3
      username: operator
      password: s3cr3t
      project_name: mesh
      project_domain_name: Default
      user_domain_name: Default
    region_name: RegionOne
    interface: public
";

    #[test]
    fn parses_the_single_context() {
        let cfg = parse_clouds_yaml(ONE_CLOUD, None).expect("parse");
        assert_eq!(cfg.cloud, "mesh");
        assert_eq!(cfg.auth_url, "http://keystone.mesh:5000/v3");
        assert_eq!(cfg.username, "operator");
        assert_eq!(cfg.password, "s3cr3t");
        assert_eq!(cfg.project_name.as_deref(), Some("mesh"));
        assert_eq!(cfg.region_name.as_deref(), Some("RegionOne"));
        assert_eq!(cfg.interface, EndpointInterface::Public);
        assert_eq!(cfg.project_domain, "Default");
    }

    #[test]
    fn debug_redacts_the_password() {
        // A config dump must never leak the credential.
        let cfg = parse_clouds_yaml(ONE_CLOUD, None).unwrap();
        let dump = format!("{cfg:?}");
        assert!(dump.contains("<redacted>"), "{dump}");
        assert!(
            !dump.contains("s3cr3t"),
            "the real password must not appear"
        );
    }

    #[test]
    fn selects_by_os_cloud_and_by_default_name() {
        let two = r"
clouds:
  alpha:
    auth: {auth_url: http://a:5000/v3, username: u, password: p}
  default:
    auth: {auth_url: http://d:5000/v3, username: u, password: p}
";
        // A named default is chosen when OS_CLOUD is unset.
        assert_eq!(parse_clouds_yaml(two, None).unwrap().cloud, "default");
        // OS_CLOUD (the `wanted` arg) overrides.
        assert_eq!(
            parse_clouds_yaml(two, Some("alpha")).unwrap().cloud,
            "alpha"
        );
    }

    #[test]
    fn an_ambiguous_file_is_a_typed_error() {
        // Two clouds, neither named default, no OS_CLOUD ⇒ never a silent pick.
        let ambiguous = r"
clouds:
  alpha:
    auth: {auth_url: http://a:5000/v3, username: u, password: p}
  beta:
    auth: {auth_url: http://b:5000/v3, username: u, password: p}
";
        let err = parse_clouds_yaml(ambiguous, None).expect_err("ambiguous must fail");
        assert!(matches!(err, ClientError::Config(_)));
        assert!(err.to_string().contains("ambiguous"), "{err}");
    }

    #[test]
    fn an_unknown_os_cloud_is_rejected() {
        let err = parse_clouds_yaml(ONE_CLOUD, Some("nope")).expect_err("unknown cloud");
        assert!(err.to_string().contains("no cloud named `nope`"), "{err}");
    }

    #[test]
    fn a_context_missing_a_required_field_is_a_typed_error() {
        let missing_pw = r"
clouds:
  mesh:
    auth:
      auth_url: http://keystone.mesh:5000/v3
      username: operator
";
        let err = parse_clouds_yaml(missing_pw, None).expect_err("missing password");
        assert!(err.to_string().contains("missing `password`"), "{err}");
    }

    #[test]
    fn an_empty_or_malformed_file_is_rejected() {
        assert!(parse_clouds_yaml("clouds: {}", None).is_err());
        assert!(parse_clouds_yaml("{[not yaml", None).is_err());
    }

    #[test]
    fn candidate_paths_include_the_standard_location() {
        let paths = candidate_paths();
        assert!(
            paths.iter().any(|p| p.ends_with("openstack/clouds.yaml")),
            "{paths:?}"
        );
        assert!(paths.contains(&PathBuf::from("/etc/openstack/clouds.yaml")));
    }

    #[test]
    fn load_from_a_fixture_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clouds.yaml");
        std::fs::write(&path, ONE_CLOUD).unwrap();
        let cfg = load_from(&path).expect("load");
        assert_eq!(cfg.auth_url, "http://keystone.mesh:5000/v3");
    }
}
