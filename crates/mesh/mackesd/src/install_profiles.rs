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
}
