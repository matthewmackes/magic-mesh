//! PLANES-24 — package mirrors (W61/W62/W63).
//!
//! A **mirror** pulls the magic-mesh GitHub-RPM channel (the Releases /
//! GitHub Pages dnf repo) into a directory on LizardFS (W61), so every
//! node serves *itself* via a `file://` baseurl off the replicated mount
//! — no HTTP tier — with the upstream as fallback (W62). The sync is a
//! scheduled one-puller job; LizardFS replicates the result (W63).
//!
//! This is the pure core: mirror configs are TOML on LizardFS
//! (`<workgroup_root>/mirrors/<name>.toml`, W88), junk-tolerant on read,
//! plus a built-in **core pack** carrying the `magic-mesh` mirror so the
//! surface ships pointed at the right channel. The `mackesd mirrors` CLI
//! verb + the Provisioning ▸ Mirrors panel render on top; the last-sync
//! marker (`<root>/mirrors/<name>/.last-sync`) reports freshness.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One package mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mirror {
    /// Stable id (the repo id + the TOML stem).
    pub name: String,
    /// Human description.
    #[serde(default)]
    pub description: String,
    /// Upstream baseurl the puller mirrors from (W61) — and the dnf
    /// fallback when the local mount is unavailable (W62).
    pub upstream: String,
    /// Whether this mirror is synced + served (W63).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

impl Mirror {
    /// The on-disk mirror directory under the workgroup root.
    #[must_use]
    pub fn local_dir(&self, workgroup_root: &Path) -> PathBuf {
        mirrors_dir(workgroup_root).join(&self.name)
    }

    /// The `file://` baseurl every node serves itself from (W62).
    #[must_use]
    pub fn file_baseurl(&self, workgroup_root: &Path) -> String {
        format!("file://{}", self.local_dir(workgroup_root).display())
    }

    /// Unix-ms of the last successful sync, read from the `.last-sync`
    /// marker the puller writes; `None` when never synced.
    #[must_use]
    pub fn last_sync_ms(&self, workgroup_root: &Path) -> Option<u64> {
        let marker = self.local_dir(workgroup_root).join(".last-sync");
        std::fs::read_to_string(marker)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
    }
}

/// The mirror-config directory (`<root>/mirrors/`).
#[must_use]
pub fn mirrors_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("mirrors")
}

/// Read every mirror TOML (junk-tolerant) plus the built-in core pack
/// (the `magic-mesh` GitHub-RPM mirror). On-disk mirrors with the same
/// `name` as a core mirror override it.
#[must_use]
pub fn load_mirrors(workgroup_root: &Path) -> Vec<Mirror> {
    let mut by_name: std::collections::BTreeMap<String, Mirror> = core_pack()
        .into_iter()
        .map(|m| (m.name.clone(), m))
        .collect();
    if let Ok(entries) = std::fs::read_dir(mirrors_dir(workgroup_root)) {
        for e in entries.filter_map(Result::ok) {
            if e.path().extension().is_some_and(|x| x == "toml") {
                if let Ok(raw) = std::fs::read_to_string(e.path()) {
                    if let Ok(m) = toml::from_str::<Mirror>(&raw) {
                        by_name.insert(m.name.clone(), m);
                    }
                }
            }
        }
    }
    by_name.into_values().collect()
}

/// The shipped mirror: the magic-mesh GitHub-RPM channel (the GitHub
/// Pages dnf repo, the COPR replacement — operator decision 2026-06-10).
#[must_use]
pub fn core_pack() -> Vec<Mirror> {
    vec![Mirror {
        name: "magic-mesh".into(),
        description: "The magic-mesh RPM channel — GitHub Releases assets + the GitHub Pages dnf repo, mirrored to LizardFS so every node serves itself.".into(),
        upstream: "https://matthewmackes.github.io/magic-mesh/fedora-$releasever-$basearch/".into(),
        enabled: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_pack_ships_the_github_rpm_mirror() {
        let pack = core_pack();
        assert_eq!(pack.len(), 1);
        assert_eq!(pack[0].name, "magic-mesh");
        assert!(pack[0].upstream.contains("matthewmackes.github.io"));
        assert!(pack[0].enabled);
    }

    #[test]
    fn file_baseurl_and_local_dir_root_under_mirrors() {
        let m = &core_pack()[0];
        let root = Path::new("/wg");
        assert_eq!(m.local_dir(root), Path::new("/wg/mirrors/magic-mesh"));
        assert_eq!(m.file_baseurl(root), "file:///wg/mirrors/magic-mesh");
    }

    #[test]
    fn last_sync_reads_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let m = &core_pack()[0];
        assert!(m.last_sync_ms(tmp.path()).is_none());
        std::fs::create_dir_all(m.local_dir(tmp.path())).unwrap();
        std::fs::write(m.local_dir(tmp.path()).join(".last-sync"), "1700000000000").unwrap();
        assert_eq!(m.last_sync_ms(tmp.path()), Some(1_700_000_000_000));
    }

    #[test]
    fn on_disk_mirror_overrides_a_core_one_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(mirrors_dir(tmp.path())).unwrap();
        std::fs::write(
            mirrors_dir(tmp.path()).join("magic-mesh.toml"),
            "name = \"magic-mesh\"\nupstream = \"https://example/repo/\"\nenabled = false\n",
        )
        .unwrap();
        let mirrors = load_mirrors(tmp.path());
        let m = mirrors.iter().find(|m| m.name == "magic-mesh").unwrap();
        assert_eq!(m.upstream, "https://example/repo/");
        assert!(!m.enabled);
        assert_eq!(mirrors.len(), 1);
    }
}
