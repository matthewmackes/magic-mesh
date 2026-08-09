//! PLANES-21 — install profiles (W56/W57/W60).
//!
//! An **install profile** is a named deployment template: a role pin, a
//! capability-tag set, the kickstart `%post` fragments it injects, and a
//! join-token slot the firstboot auto-join fills (W60). One image
//! carries every profile; the boot menu picks one at install (W57).
//!
//! This is the pure core: profiles are TOML on the Syncthing-replicated share
//! (`<workgroup_root>/profiles/*.toml`, W88 — fleet state is TOML dirs +
//! typed Bus verbs), junk-tolerant on read, plus a built-in **core pack**
//! mapping the two deployment roles (Lighthouse or Workstation, §5) to
//! their stock profiles so the surface
//! is never empty. The
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
    /// Deployment role this profile pins (`lighthouse` or `workstation`, §5).
    /// Retired Server and lighthouse subclasses are rejected.
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

/// Install profiles are small replicated TOML records. Keep peer-controlled
/// input bounded before `toml` materializes it into an owned document.
const MAX_PROFILE_BYTES: usize = 256 * 1024;

/// Read one replicated profile through the descriptor that will be consumed.
/// Reject final symlinks, blocking special files, oversized input, and invalid
/// UTF-8 before the TOML parser sees peer-controlled bytes.
fn read_bounded_profile(path: &Path) -> Option<String> {
    use std::io::Read as _;

    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.file_type().is_file() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_PROFILE_BYTES as u64 {
        return None;
    }

    let capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_PROFILE_BYTES)
        .min(MAX_PROFILE_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take((MAX_PROFILE_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_PROFILE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Read every profile TOML (junk-tolerant) plus the built-in core pack
/// (the two role profiles). On-disk profiles with the same `name` as a
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
                if let Some(raw) = read_bounded_profile(&e.path()) {
                    if let Ok(p) = toml::from_str::<InstallProfile>(&raw) {
                        // Retired media/file-sharing lighthouse profiles are
                        // ignored rather than reintroduced from replicated
                        // state.
                        if validate_profile(&p).is_ok() {
                            by_name.insert(p.name.clone(), p);
                        }
                    }
                }
            }
        }
    }
    by_name.into_values().collect()
}

/// The two shipped deployment profiles (§5). A headless machine is a
/// Workstation carrying the `headless` tag, not a third role. Workstations
/// auto-join so a USB/ISO install enrols hands-free; the founding Lighthouse
/// establishes the mesh and therefore cannot auto-join it.
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
            name: "workstation".into(),
            description:
                "The Construct Workstation stack; add headless when no local display is present."
                    .into(),
            role: "workstation".into(),
            tags: BTreeSet::from(["execution".to_string()]),
            ks_fragments: vec!["role-workstation".into(), "construct-desktop".into()],
            auto_join: true,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────
// W56 — the form-edit write side. The Provisioning ▸ Install Profiles
// panel (and `mackesd profiles set`) build an InstallProfile and persist
// it as `<root>/profiles/<name>.toml`; Syncthing replicates it, and
// load_profiles picks it up (overriding a same-named core profile).
// Validated up front so a typo'd role/tag never reaches an installer.
// ─────────────────────────────────────────────────────────────────

/// The two deployment roles a profile may pin (§5).
pub const VALID_ROLES: [&str; 2] = ["lighthouse", "workstation"];

/// The capability tags a profile may carry. Kept in lock-step with
/// [`mackes_mesh_types::cap_tags::CapabilityTag`] — a profile tag that the
/// typed vocabulary can't parse would never gate.
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
    reject_symlinked_directory_components(&dir)
        .map_err(|e| ProfileWriteError::Io(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| ProfileWriteError::Io(e.to_string()))?;
    reject_symlinked_directory_components(&dir)
        .map_err(|e| ProfileWriteError::Io(e.to_string()))?;
    let body =
        toml::to_string_pretty(profile).map_err(|e| ProfileWriteError::Serialize(e.to_string()))?;
    let path = dir.join(format!("{}.toml", profile.name));
    write_atomic_public(&path, body.as_bytes())
        .map_err(|e| ProfileWriteError::Io(e.to_string()))?;
    Ok(path)
}

/// Refuse to traverse a replicated directory symlink while preparing a profile
/// destination. The final TOML is replaced by rename only after its complete
/// body is synced, so a bad peer cannot redirect the write through a parent or
/// expose a truncated profile.
fn reject_symlinked_directory_components(path: &Path) -> std::io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("refusing symlinked profile directory {}", current.display()),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!(
                        "profile path component is not a directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Crash-durable public-state replacement for a profile TOML. The unique
/// `create_new` sibling cannot follow an attacker-created temp symlink; the
/// final rename replaces a destination symlink without touching its target.
fn write_atomic_public(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid profile filename {}", path.display()),
            )
        })?;
    let (tmp, mut file) = (0..16)
        .find_map(|_| {
            let candidate = parent.join(format!(
                ".{leaf}.tmp.{}.{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&candidate)
            {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .unwrap_or_else(|| {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("temporary profile collisions for {}", path.display()),
            ))
        })?;

    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
    fn core_pack_covers_every_deployment_role() {
        let pack = core_pack();
        let roles: BTreeSet<&str> = pack.iter().map(|p| p.role.as_str()).collect();
        assert_eq!(roles, BTreeSet::from(["lighthouse", "workstation"]));
        // Every core profile validates (role + tags + name).
        for p in &pack {
            validate_profile(p).unwrap_or_else(|e| panic!("core profile {} invalid: {e}", p.name));
        }
    }

    #[test]
    fn retired_server_and_hypervisor_profiles_are_absent() {
        let pack = core_pack();
        assert!(pack.iter().all(|profile| profile.name != "server"));
        assert!(pack.iter().all(|profile| profile.name != "hypervisor"));
        assert_eq!(
            mackes_mesh_types::cap_tags::CapabilityTag::parse("hypervisor"),
            None
        );
    }

    #[test]
    fn valid_tags_match_the_typed_vocabulary() {
        // A profile tag the typed CapabilityTag can't parse would validate
        // here but never gate at runtime; keep the two lists in lock-step.
        let typed: Vec<&str> = mackes_mesh_types::cap_tags::CapabilityTag::ALL
            .iter()
            .filter(|t| **t != mackes_mesh_types::cap_tags::CapabilityTag::Media)
            .map(|t| t.as_str())
            .collect();
        assert_eq!(VALID_TAGS.to_vec(), typed);
    }

    #[test]
    fn workstation_is_execution_capable_and_displayed_by_default() {
        let pack = core_pack();
        let ws = pack.iter().find(|p| p.name == "workstation").unwrap();
        assert!(ws.tags.contains("execution"));
        assert!(!ws.tags.contains("headless"));
    }

    #[test]
    fn retired_lighthouse_media_profile_is_not_selectable() {
        let pack = core_pack();
        assert!(pack.iter().all(|p| p.name != "lighthouse-media"));
        let retired = InstallProfile {
            name: "lighthouse-media".into(),
            description: "retired".into(),
            role: "lighthouse_media".into(),
            tags: BTreeSet::from(["media".to_string()]),
            ks_fragments: vec!["media-lighthouse".into()],
            auto_join: true,
        };
        assert!(matches!(
            validate_profile(&retired),
            Err(ProfileWriteError::BadRole(_))
        ));
    }

    #[test]
    fn on_disk_profile_overrides_a_core_one_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(profiles_dir(tmp.path())).unwrap();
        std::fs::write(
            profiles_dir(tmp.path()).join("workstation.toml"),
            "name = \"workstation\"\nrole = \"workstation\"\ndescription = \"custom\"\nauto_join = false\n",
        )
        .unwrap();
        let profiles = load_profiles(tmp.path());
        let workstation = profiles.iter().find(|p| p.name == "workstation").unwrap();
        assert_eq!(workstation.description, "custom");
        assert!(!workstation.auto_join);
        // Still exactly the core count (override, not duplicate).
        assert_eq!(profiles.len(), core_pack().len());
    }

    #[test]
    fn load_profiles_includes_core_pack_when_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_profiles(tmp.path()).len(), core_pack().len());
    }

    #[cfg(unix)]
    #[test]
    fn load_profiles_skips_hostile_replicated_leaves_and_keeps_core_fallback() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dir = profiles_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();

        // A final symlink must not turn a replicated override into an escape
        // or replace the built-in profile with data from outside the share.
        let escaped = tmp.path().join("escaped-workstation.toml");
        std::fs::write(
            &escaped,
            "name = \"workstation\"\nrole = \"workstation\"\ndescription = \"escaped\"\n",
        )
        .unwrap();
        symlink(&escaped, dir.join("workstation.toml")).unwrap();

        // A directory is a non-regular leaf even though its name looks like
        // a profile. O_NONBLOCK also keeps special-file opens from blocking.
        std::fs::create_dir(dir.join("directory.toml")).unwrap();

        // Invalid UTF-8 is rejected before TOML materialization.
        std::fs::write(dir.join("invalid-utf8.toml"), [0xff, 0xfe, 0xfd]).unwrap();

        // Read at most the profile ceiling plus one sentinel byte.
        std::fs::write(
            dir.join("oversized.toml"),
            vec![b'x'; MAX_PROFILE_BYTES.saturating_add(1)],
        )
        .unwrap();

        // Existing junk tolerance remains intact for malformed TOML.
        std::fs::write(dir.join("junk.toml"), "not = [valid TOML").unwrap();

        let profiles = load_profiles(tmp.path());
        assert_eq!(profiles.len(), core_pack().len());
        let workstation = profiles
            .iter()
            .find(|profile| profile.name == "workstation");
        assert_eq!(
            workstation,
            core_pack()
                .iter()
                .find(|profile| profile.name == "workstation")
        );
        assert!(!profiles.iter().any(|profile| {
            matches!(
                profile.name.as_str(),
                "directory" | "invalid-utf8" | "oversized" | "junk"
            )
        }));
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
        // It comes back out of load_profiles alongside the two core ones.
        let loaded = load_profiles(tmp.path());
        let got = loaded
            .iter()
            .find(|x| x.name == "edge-relay")
            .expect("loaded");
        assert_eq!(got, &p);
        assert_eq!(loaded.len(), core_pack().len() + 1);
    }

    #[test]
    fn write_profile_overwrites_a_core_one_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = core_pack()
            .into_iter()
            .find(|p| p.name == "workstation")
            .unwrap();
        p.description = "house style".into();
        write_profile(&p, tmp.path()).expect("write");
        let loaded = load_profiles(tmp.path());
        assert_eq!(loaded.len(), core_pack().len(), "override, not duplicate");
        assert_eq!(
            loaded
                .iter()
                .find(|x| x.name == "workstation")
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
            role: "workstation".into(),
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
            role: "workstation".into(),
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
        assert_eq!(load_profiles(tmp.path()).len(), core_pack().len());
    }

    #[cfg(unix)]
    #[test]
    fn write_profile_replaces_final_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let profile = InstallProfile {
            name: "edge-relay".into(),
            description: "safe replacement".into(),
            role: "workstation".into(),
            tags: BTreeSet::from(["execution".to_string()]),
            ks_fragments: vec![],
            auto_join: false,
        };
        let dir = profiles_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        let victim = tmp.path().join("victim.toml");
        std::fs::write(&victim, b"sentinel").unwrap();
        symlink(&victim, dir.join("edge-relay.toml")).unwrap();

        write_profile(&profile, tmp.path()).expect("replace safely");

        assert_eq!(std::fs::read(&victim).unwrap(), b"sentinel");
        assert_eq!(
            load_profiles(tmp.path())
                .into_iter()
                .find(|candidate| candidate.name == profile.name),
            Some(profile)
        );
        assert!(!std::fs::symlink_metadata(dir.join("edge-relay.toml"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp.")));
    }

    #[cfg(unix)]
    #[test]
    fn write_profile_rejects_symlinked_parent_before_escape() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), profiles_dir(tmp.path())).unwrap();

        let profile = InstallProfile {
            name: "edge-relay".into(),
            description: String::new(),
            role: "workstation".into(),
            tags: BTreeSet::new(),
            ks_fragments: vec![],
            auto_join: false,
        };
        let error = write_profile(&profile, tmp.path())
            .expect_err("symlinked profile root must be rejected");
        assert!(matches!(error, ProfileWriteError::Io(_)));
        assert!(!outside.path().join("edge-relay.toml").exists());
    }
}
