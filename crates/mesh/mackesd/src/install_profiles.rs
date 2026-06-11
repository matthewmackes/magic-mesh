//! PLANES-21 — install profiles (W56/W57/W60).
//!
//! An **install profile** is a named deployment template: a role pin, a
//! capability-tag set, the kickstart `%post` fragments it injects, and a
//! join-token slot the firstboot auto-join fills (W60). One image
//! carries every profile; the boot menu picks one at install (W57).
//!
//! This is the pure core: profiles are TOML on LizardFS
//! (`<workgroup_root>/profiles/*.toml`, W88 — fleet state is TOML dirs +
//! typed Bus verbs), junk-tolerant on read, plus a built-in **core pack**
//! mapping the three deployment roles (Lighthouse ⊂ Server ⊂ Workstation,
//! §5) to their stock profiles so the surface is never empty. The
//! `mackesd profiles` CLI verb + the Provisioning ▸ Install Profiles
//! panel render on top.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One install profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallProfile {
    /// Stable id (the boot-menu label + the TOML stem).
    pub name: String,
    /// Human description shown on the boot menu / panel.
    #[serde(default)]
    pub description: String,
    /// Deployment role this profile pins (`lighthouse`|`server`|
    /// `workstation`, §5).
    pub role: String,
    /// Capability tags applied at firstboot (hop|execution|headless, W82).
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Kickstart `%post` fragment ids this profile injects (W56).
    #[serde(default)]
    pub ks_fragments: Vec<String>,
    /// Whether the firstboot auto-join slot is filled with a single-use
    /// bearer (W60). The token itself is never stored in the profile —
    /// only whether the image bakes one in.
    #[serde(default)]
    pub auto_join: bool,
}

/// The install-profiles directory (`<root>/profiles/`).
#[must_use]
pub fn profiles_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("profiles")
}

/// Read every profile TOML (junk-tolerant) plus the built-in core pack
/// (the three role profiles). On-disk profiles with the same `name` as a
/// core profile override it.
#[must_use]
pub fn load_profiles(workgroup_root: &Path) -> Vec<InstallProfile> {
    let mut by_name: std::collections::BTreeMap<String, InstallProfile> = core_pack()
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();
    if let Ok(entries) = std::fs::read_dir(profiles_dir(workgroup_root)) {
        for e in entries.filter_map(Result::ok) {
            if e.path().extension().is_some_and(|x| x == "toml") {
                if let Ok(raw) = std::fs::read_to_string(e.path()) {
                    if let Ok(p) = toml::from_str::<InstallProfile>(&raw) {
                        by_name.insert(p.name.clone(), p);
                    }
                }
            }
        }
    }
    by_name.into_values().collect()
}

/// The shipped profiles — one per deployment role (§5). Server and
/// Workstation are execution-capable; the headless Server omits the
/// desktop; the Workstation is the Cosmic desktop. All auto-join so a
/// USB/ISO install enrols hands-free (W60).
#[must_use]
pub fn core_pack() -> Vec<InstallProfile> {
    vec![
        InstallProfile {
            name: "lighthouse".into(),
            description:
                "The founding relay + CA + leader control plane — the first node in a new mesh."
                    .into(),
            role: "lighthouse".into(),
            tags: BTreeSet::from(["hop".to_string()]),
            ks_fragments: vec!["role-lighthouse".into(), "nebula-lighthouse".into()],
            auto_join: false,
        },
        InstallProfile {
            name: "server".into(),
            description:
                "Everything a lighthouse runs, plus fleet automation + LizardFS storage. Headless."
                    .into(),
            role: "server".into(),
            tags: BTreeSet::from(["execution".to_string(), "headless".to_string()]),
            ks_fragments: vec!["role-server".into(), "lizardfs".into()],
            auto_join: true,
        },
        InstallProfile {
            name: "workstation".into(),
            description:
                "Everything a server runs, plus the Cosmic desktop, voice, media, and KDC.".into(),
            role: "workstation".into(),
            tags: BTreeSet::from(["execution".to_string()]),
            ks_fragments: vec!["role-workstation".into(), "cosmic-desktop".into()],
            auto_join: true,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────
// W56 — the form-edit write side. The Provisioning ▸ Install Profiles
// panel (and `mackesd profiles set`) build an InstallProfile and persist
// it as `<root>/profiles/<name>.toml`; LizardFS replicates it, and
// load_profiles picks it up (overriding a same-named core profile).
// Validated up front so a typo'd role/tag never reaches an installer.
// ─────────────────────────────────────────────────────────────────

/// The deployment roles a profile may pin (§5, Lighthouse ⊂ Server ⊂
/// Workstation).
pub const VALID_ROLES: [&str; 3] = ["lighthouse", "server", "workstation"];

/// The capability tags a profile may carry (W82).
pub const VALID_TAGS: [&str; 3] = ["hop", "execution", "headless"];

/// Why a profile write was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileWriteError {
    /// `name` was empty or not a `[a-z0-9-]+` slug (it becomes a filename).
    BadName(String),
    /// `role` is not one of [`VALID_ROLES`].
    BadRole(String),
    /// A tag is not one of [`VALID_TAGS`].
    BadTag(String),
    /// TOML serialization failed (practically never).
    Serialize(String),
    /// Filesystem write failed.
    Io(String),
}

impl std::fmt::Display for ProfileWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName(n) => write!(f, "invalid profile name '{n}' (need a [a-z0-9-]+ slug)"),
            Self::BadRole(r) => {
                write!(f, "invalid role '{r}' (expected one of {VALID_ROLES:?})")
            }
            Self::BadTag(t) => write!(f, "invalid tag '{t}' (expected one of {VALID_TAGS:?})"),
            Self::Serialize(e) => write!(f, "serialize profile: {e}"),
            Self::Io(e) => write!(f, "write profile: {e}"),
        }
    }
}
impl std::error::Error for ProfileWriteError {}

/// A name is a filesystem-safe kebab slug — it becomes `<name>.toml`.
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Validate a profile's role + tags + name without writing it. Returns
/// the first problem found.
///
/// # Errors
/// [`ProfileWriteError`] naming the offending field.
pub fn validate_profile(p: &InstallProfile) -> Result<(), ProfileWriteError> {
    if !is_valid_name(&p.name) {
        return Err(ProfileWriteError::BadName(p.name.clone()));
    }
    if !VALID_ROLES.contains(&p.role.as_str()) {
        return Err(ProfileWriteError::BadRole(p.role.clone()));
    }
    if let Some(bad) = p.tags.iter().find(|t| !VALID_TAGS.contains(&t.as_str())) {
        return Err(ProfileWriteError::BadTag(bad.clone()));
    }
    Ok(())
}

/// Persist `profile` as `<root>/profiles/<name>.toml` after validating it.
/// Overwrites an existing same-named profile (the intended customize/
/// override path — load_profiles lets an on-disk profile shadow a core
/// one). Returns the path written.
///
/// # Errors
/// [`ProfileWriteError`] on validation, serialization, or IO failure.
pub fn write_profile(
    profile: &InstallProfile,
    workgroup_root: &Path,
) -> Result<PathBuf, ProfileWriteError> {
    validate_profile(profile)?;
    let dir = profiles_dir(workgroup_root);
    std::fs::create_dir_all(&dir).map_err(|e| ProfileWriteError::Io(e.to_string()))?;
    let body =
        toml::to_string_pretty(profile).map_err(|e| ProfileWriteError::Serialize(e.to_string()))?;
    let path = dir.join(format!("{}.toml", profile.name));
    std::fs::write(&path, body).map_err(|e| ProfileWriteError::Io(e.to_string()))?;
    Ok(path)
}

/// Delete an on-disk profile's TOML by name. A core profile has no TOML,
/// so deleting one is a no-op that reverts to the built-in. Returns true
/// when a file was actually removed.
///
/// # Errors
/// [`ProfileWriteError::Io`] on a filesystem error other than not-found.
pub fn delete_profile(name: &str, workgroup_root: &Path) -> Result<bool, ProfileWriteError> {
    let path = profiles_dir(workgroup_root).join(format!("{name}.toml"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(ProfileWriteError::Io(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_pack_has_one_profile_per_role() {
        let pack = core_pack();
        let roles: BTreeSet<&str> = pack.iter().map(|p| p.role.as_str()).collect();
        assert_eq!(
            roles,
            BTreeSet::from(["lighthouse", "server", "workstation"])
        );
    }

    #[test]
    fn server_is_headless_execution_workstation_is_not() {
        let pack = core_pack();
        let server = pack.iter().find(|p| p.name == "server").unwrap();
        assert!(server.tags.contains("headless") && server.tags.contains("execution"));
        let ws = pack.iter().find(|p| p.name == "workstation").unwrap();
        assert!(!ws.tags.contains("headless"));
    }

    #[test]
    fn on_disk_profile_overrides_a_core_one_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(profiles_dir(tmp.path())).unwrap();
        std::fs::write(
            profiles_dir(tmp.path()).join("server.toml"),
            "name = \"server\"\nrole = \"server\"\ndescription = \"custom\"\nauto_join = false\n",
        )
        .unwrap();
        let profiles = load_profiles(tmp.path());
        let server = profiles.iter().find(|p| p.name == "server").unwrap();
        assert_eq!(server.description, "custom");
        assert!(!server.auto_join);
        // Still exactly three (override, not duplicate).
        assert_eq!(profiles.len(), 3);
    }

    #[test]
    fn load_profiles_includes_core_pack_when_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_profiles(tmp.path()).len(), 3);
    }

    // ---- W56 write side -----------------------------------------

    #[test]
    fn write_profile_round_trips_through_load() {
        let tmp = tempfile::tempdir().unwrap();
        let p = InstallProfile {
            name: "edge-relay".into(),
            description: "A custom hop-only relay".into(),
            role: "lighthouse".into(),
            tags: BTreeSet::from(["hop".to_string()]),
            ks_fragments: vec!["role-lighthouse".into()],
            auto_join: false,
        };
        let path = write_profile(&p, tmp.path()).expect("write");
        assert_eq!(path, profiles_dir(tmp.path()).join("edge-relay.toml"));
        // It comes back out of load_profiles alongside the 3 core ones.
        let loaded = load_profiles(tmp.path());
        let got = loaded
            .iter()
            .find(|x| x.name == "edge-relay")
            .expect("loaded");
        assert_eq!(got, &p);
        assert_eq!(loaded.len(), 4);
    }

    #[test]
    fn write_profile_overwrites_a_core_one_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = core_pack()
            .into_iter()
            .find(|p| p.name == "server")
            .unwrap();
        p.description = "house style".into();
        write_profile(&p, tmp.path()).expect("write");
        let loaded = load_profiles(tmp.path());
        assert_eq!(loaded.len(), 3, "override, not duplicate");
        assert_eq!(
            loaded
                .iter()
                .find(|x| x.name == "server")
                .unwrap()
                .description,
            "house style"
        );
    }

    #[test]
    fn write_profile_rejects_bad_role_name_and_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let base = || InstallProfile {
            name: "ok".into(),
            description: String::new(),
            role: "server".into(),
            tags: BTreeSet::new(),
            ks_fragments: vec![],
            auto_join: false,
        };
        let mut bad_role = base();
        bad_role.role = "overlord".into();
        assert!(matches!(
            write_profile(&bad_role, tmp.path()),
            Err(ProfileWriteError::BadRole(_))
        ));
        let mut bad_name = base();
        bad_name.name = "../escape".into();
        assert!(matches!(
            write_profile(&bad_name, tmp.path()),
            Err(ProfileWriteError::BadName(_))
        ));
        let mut bad_tag = base();
        bad_tag.tags = BTreeSet::from(["teleport".to_string()]);
        assert!(matches!(
            write_profile(&bad_tag, tmp.path()),
            Err(ProfileWriteError::BadTag(_))
        ));
        // None of the rejects left a file behind.
        assert!(!profiles_dir(tmp.path()).join("ok.toml").exists());
    }

    #[test]
    fn delete_profile_removes_on_disk_and_noops_on_core() {
        let tmp = tempfile::tempdir().unwrap();
        let p = InstallProfile {
            name: "scratch".into(),
            description: String::new(),
            role: "server".into(),
            tags: BTreeSet::new(),
            ks_fragments: vec![],
            auto_join: false,
        };
        write_profile(&p, tmp.path()).unwrap();
        assert!(delete_profile("scratch", tmp.path()).unwrap(), "removed");
        assert!(
            !delete_profile("scratch", tmp.path()).unwrap(),
            "already gone"
        );
        // A core profile has no TOML → delete is a clean no-op (false).
        assert!(!delete_profile("lighthouse", tmp.path()).unwrap());
        assert_eq!(load_profiles(tmp.path()).len(), 3);
    }
}
