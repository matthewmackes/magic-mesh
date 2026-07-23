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
//! mapping the deployment roles (Lighthouse ⊂ Server ⊂ Workstation, §5) to
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
    /// Deployment role this profile pins (`lighthouse`|`server`|`workstation`,
    /// §5). The retired `lighthouse_media` role is rejected.
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

/// The shipped profiles — the deployment roles (§5) plus the
/// XCP-ng `hypervisor` profile (DATACENTER-17).
///
/// Server and Workstation are execution-capable; the headless Server omits the desktop; the
/// Workstation is the Cosmic desktop. The
/// `hypervisor` profile pins the Server tier (PeerRole flattens to
/// Host/Peer, so the dom0 is surfaced via the `hypervisor` capability
/// tag, not a 4th cert role) and joins as a static-Nebula member. The
/// role profiles auto-join so a USB/ISO install enrols hands-free (W60);
/// the hypervisor is provisioned by `onboard-xcp-host.sh` (static
/// `nebula` on a locked-down dom0), so it does not bake the firstboot
/// auto-join slot.
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
                "Everything a lighthouse runs, plus fleet automation + a Syncthing storage replica. Headless."
                    .into(),
            role: "server".into(),
            tags: BTreeSet::from(["execution".to_string(), "headless".to_string()]),
            ks_fragments: vec!["role-server".into()],
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
        InstallProfile {
            name: "hypervisor".into(),
            description: "XCP-ng dom0 joined as a static-Nebula mesh member".into(),
            // PeerRole flattens to Host/Peer (open-mesh), so a dom0 maps to
            // the Server/Host tier; `hypervisor` is the capability tag that
            // makes it first-class in the roster (DATACENTER-17).
            role: "server".into(),
            tags: BTreeSet::from(["hypervisor".to_string()]),
            ks_fragments: vec!["role-hypervisor".into(), "nebula-static".into()],
            // Joined via onboard-xcp-host.sh on a locked-down dom0 (static
            // `nebula`, not the Fedora firstboot auto-join flow).
            auto_join: false,
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

/// The deployment roles a profile may pin (§5, Lighthouse ⊂ Server ⊂
/// Workstation). The former media-lighthouse subclass is retired.
pub const VALID_ROLES: [&str; 3] = ["lighthouse", "server", "workstation"];

/// The capability tags a profile may carry (W82; `hypervisor` added by
/// DATACENTER-17 for the XCP-ng dom0 profile). Kept in lock-step with
/// [`mackes_mesh_types::cap_tags::CapabilityTag`] — a profile tag that the
/// typed vocabulary can't parse would never gate.
pub const VALID_TAGS: [&str; 4] = ["hop", "execution", "headless", "hypervisor"];

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
    fn core_pack_covers_every_deployment_role() {
        let pack = core_pack();
        let roles: BTreeSet<&str> = pack.iter().map(|p| p.role.as_str()).collect();
        // hypervisor pins the server tier; retired media lighthouse profiles
        // are intentionally absent.
        assert_eq!(
            roles,
            BTreeSet::from(["lighthouse", "server", "workstation"])
        );
        // Every core profile validates (role + tags + name).
        for p in &pack {
            validate_profile(p).unwrap_or_else(|e| panic!("core profile {} invalid: {e}", p.name));
        }
    }

    #[test]
    fn hypervisor_profile_is_a_server_tier_static_nebula_member() {
        let pack = core_pack();
        let hv = pack
            .iter()
            .find(|p| p.name == "hypervisor")
            .expect("DATACENTER-17 — hypervisor profile present");
        // Server tier + the hypervisor capability tag (not a 4th cert role).
        assert_eq!(hv.role, "server");
        assert!(hv.tags.contains("hypervisor"));
        // Static-Nebula join on a locked-down dom0: no firstboot auto-join.
        assert!(!hv.auto_join);
        assert_eq!(hv.ks_fragments, vec!["role-hypervisor", "nebula-static"]);
        // The tag round-trips through the typed vocabulary the writer gates on.
        assert_eq!(
            mackes_mesh_types::cap_tags::CapabilityTag::parse("hypervisor"),
            Some(mackes_mesh_types::cap_tags::CapabilityTag::Hypervisor),
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
    fn server_is_headless_execution_workstation_is_not() {
        let pack = core_pack();
        let server = pack.iter().find(|p| p.name == "server").unwrap();
        assert!(server.tags.contains("headless") && server.tags.contains("execution"));
        let ws = pack.iter().find(|p| p.name == "workstation").unwrap();
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
            profiles_dir(tmp.path()).join("server.toml"),
            "name = \"server\"\nrole = \"server\"\ndescription = \"custom\"\nauto_join = false\n",
        )
        .unwrap();
        let profiles = load_profiles(tmp.path());
        let server = profiles.iter().find(|p| p.name == "server").unwrap();
        assert_eq!(server.description, "custom");
        assert!(!server.auto_join);
        // Still exactly the core count (override, not duplicate).
        assert_eq!(profiles.len(), core_pack().len());
    }

    #[test]
    fn load_profiles_includes_core_pack_when_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_profiles(tmp.path()).len(), core_pack().len());
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
        assert_eq!(loaded.len(), core_pack().len() + 1);
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
        assert_eq!(loaded.len(), core_pack().len(), "override, not duplicate");
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
        assert_eq!(load_profiles(tmp.path()).len(), core_pack().len());
    }
}
