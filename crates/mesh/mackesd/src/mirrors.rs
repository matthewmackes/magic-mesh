//! PLANES-24 — package mirrors (W61/W62/W63).
//!
//! A **mirror** pulls the magic-mesh GitHub-RPM channel (the Releases /
//! GitHub Pages dnf repo) into a directory on the Syncthing-replicated share
//! (W61), so every node serves *itself* via a `file://` baseurl off the shared
//! dir — no HTTP tier — with the upstream as fallback (W62). The sync is a
//! scheduled one-puller job; Syncthing replicates the result (W63).
//!
//! This is the pure core: mirror configs are TOML on the Syncthing-replicated
//! share (`<workgroup_root>/mirrors/<name>.toml`, W88), junk-tolerant on read,
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
        description: "The magic-mesh RPM channel — GitHub Releases assets + the GitHub Pages dnf repo, mirrored to the Syncthing share so every node serves itself.".into(),
        upstream: "https://matthewmackes.github.io/magic-mesh/fedora-$releasever-$basearch/".into(),
        enabled: true,
    }]
}

// ─────────────────────────────────────────────────────────────────
// W63 — the one-puller sync. A node pulls the upstream dnf repo into
// the mirror's local_dir + (re)builds repo metadata with createrepo_c,
// then stamps `.last-sync`; Syncthing replicates the result so every
// other node serves it from the `file://` share without pulling again.
// ─────────────────────────────────────────────────────────────────

/// What one successful sync produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// The mirror that was synced.
    pub name: String,
    /// Where the RPMs + repodata landed on the replicated mount.
    pub local_dir: PathBuf,
    /// Count of RPMs present after the pull.
    pub rpm_count: u32,
    /// Unix-ms stamped into `.last-sync`.
    pub synced_at_ms: u64,
    /// The `file://` baseurl nodes now serve themselves from (W62).
    pub served_baseurl: String,
}

/// Why a sync did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorSyncError {
    /// The mirror is `enabled = false` — the puller skips it.
    Disabled,
    /// Creating the local mirror dir failed.
    MkDir(String),
    /// `dnf reposync` failed (binary missing, network, bad upstream).
    Reposync(String),
    /// `createrepo_c` failed (binary missing, bad tree).
    Createrepo(String),
    /// Writing the `.last-sync` marker failed.
    Marker(String),
}

impl std::fmt::Display for MirrorSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "mirror is disabled (enabled = false)"),
            Self::MkDir(e) => write!(f, "create mirror dir: {e}"),
            Self::Reposync(e) => write!(f, "dnf reposync: {e}"),
            Self::Createrepo(e) => write!(f, "createrepo_c: {e}"),
            Self::Marker(e) => write!(f, "write .last-sync: {e}"),
        }
    }
}
impl std::error::Error for MirrorSyncError {}

/// The two subprocess steps a sync shells out — behind a trait so the
/// sync orchestration (dir creation, marker write, report assembly) is
/// unit-tested with a mock, exactly as the CA flow mocks `nebula-cert`
/// via [`crate::ca::NebulaCertBackend`]. `reposync` returns the count of
/// RPMs present in `dest` after the pull.
pub trait MirrorSyncRunner {
    /// Mirror `upstream` (a dnf repo baseurl) into `dest`. Returns the
    /// number of `.rpm` files present afterwards.
    ///
    /// # Errors
    /// A human-readable string on subprocess failure.
    fn reposync(&self, name: &str, upstream: &str, dest: &Path) -> Result<u32, String>;
    /// (Re)build `repodata/` over `dir` with `createrepo_c`.
    ///
    /// # Errors
    /// A human-readable string on subprocess failure.
    fn createrepo(&self, dir: &Path) -> Result<(), String>;
}

/// Production runner: shells the real Fedora tools (`Requires: dnf-plugins-core`
/// for `dnf reposync`, `createrepo_c` for the indexer — both pulled by the RPM).
pub struct SubprocessSync;

impl MirrorSyncRunner for SubprocessSync {
    fn reposync(&self, name: &str, upstream: &str, dest: &Path) -> Result<u32, String> {
        // `--repofrompath` defines an ad-hoc repo so no /etc/yum.repos.d
        // file is needed; `--norepopath` drops the packages straight into
        // `dest` (not `dest/<name>`) so the `file://` baseurl resolves.
        let out = std::process::Command::new("dnf")
            .arg("reposync")
            .arg(format!("--repofrompath={name},{upstream}"))
            .arg(format!("--repoid={name}"))
            .arg("--download-path")
            .arg(dest)
            .arg("--norepopath")
            .arg("--delete")
            .output()
            .map_err(|e| format!("spawn dnf reposync: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(count_rpms(dest))
    }

    fn createrepo(&self, dir: &Path) -> Result<(), String> {
        let out = std::process::Command::new("createrepo_c")
            .arg("--update")
            .arg(dir)
            .output()
            .map_err(|e| format!("spawn createrepo_c: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// Count `.rpm` files directly under `dir` (non-recursive — `--norepopath`
/// drops them flat). Missing dir → 0.
#[must_use]
pub fn count_rpms(dir: &Path) -> u32 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "rpm"))
                .count() as u32
        })
        .unwrap_or(0)
}

/// Sync one mirror: create its dir, pull the upstream repo, rebuild the
/// metadata, stamp `.last-sync` with `now_ms`. `now_ms` is injected (not
/// read from the clock here) so the orchestration is deterministically
/// testable. A disabled mirror is refused before any work.
///
/// # Errors
/// [`MirrorSyncError`] at the first failing step.
pub fn sync_mirror<R: MirrorSyncRunner + ?Sized>(
    runner: &R,
    mirror: &Mirror,
    workgroup_root: &Path,
    now_ms: u64,
) -> Result<SyncReport, MirrorSyncError> {
    if !mirror.enabled {
        return Err(MirrorSyncError::Disabled);
    }
    let dir = mirror.local_dir(workgroup_root);
    std::fs::create_dir_all(&dir).map_err(|e| MirrorSyncError::MkDir(e.to_string()))?;
    let rpm_count = runner
        .reposync(&mirror.name, &mirror.upstream, &dir)
        .map_err(MirrorSyncError::Reposync)?;
    runner
        .createrepo(&dir)
        .map_err(MirrorSyncError::Createrepo)?;
    // Stamp freshness LAST — only a fully-indexed mirror is "synced".
    std::fs::write(dir.join(".last-sync"), now_ms.to_string())
        .map_err(|e| MirrorSyncError::Marker(e.to_string()))?;
    Ok(SyncReport {
        name: mirror.name.clone(),
        local_dir: dir.clone(),
        rpm_count,
        synced_at_ms: now_ms,
        served_baseurl: mirror.file_baseurl(workgroup_root),
    })
}

// ─────────────────────────────────────────────────────────────────
// W62 — flip a node to self-serve. Render the dnf `.repo` so its
// baseurl is the local `file://` mirror FIRST, with the upstream as a
// fallback line: dnf tries each baseurl in order, so a node reads from
// its Syncthing-replicated share and only falls back to GitHub when the
// share is unavailable. No HTTP tier.
// ─────────────────────────────────────────────────────────────────

/// The canonical dnf repo-config directory a node serves from.
pub const DEFAULT_REPO_DIR: &str = "/etc/yum.repos.d";

/// Render the dnf `.repo` INI for `mirror`: `file://` local baseurl
/// first, upstream second (the W62 self-serve-with-fallback order).
#[must_use]
pub fn render_dnf_repo(mirror: &Mirror, workgroup_root: &Path) -> String {
    let descr = if mirror.description.is_empty() {
        mirror.name.clone()
    } else {
        mirror.description.clone()
    };
    // dnf accepts multiple baseurls (whitespace/newline-separated) and
    // tries them in order — local mount first, GitHub upstream as the
    // fallback. The continuation line is indented per INI convention.
    format!(
        "[{name}]\n\
         name={descr}\n\
         baseurl={local}\n\
         \x20      {upstream}\n\
         enabled={enabled}\n\
         gpgcheck=0\n\
         metadata_expire=300\n",
        name = mirror.name,
        local = mirror.file_baseurl(workgroup_root),
        upstream = mirror.upstream,
        enabled = u8::from(mirror.enabled),
    )
}

/// Write `mirror`'s `.repo` into `repo_dir` (e.g. `/etc/yum.repos.d`).
/// Returns the path written. `repo_dir` is a parameter (not the const)
/// so tests redirect into a tempdir instead of touching `/etc`.
///
/// # Errors
/// [`std::io::Error`] when the dir can't be created or the file written.
pub fn write_dnf_repo(
    mirror: &Mirror,
    workgroup_root: &Path,
    repo_dir: &Path,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(repo_dir)?;
    let path = repo_dir.join(format!("mackes-mirror-{}.repo", mirror.name));
    std::fs::write(&path, render_dnf_repo(mirror, workgroup_root))?;
    Ok(path)
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

    // ---- W63 sync ------------------------------------------------

    /// Mock runner: records the calls, writes a couple of fake RPMs so
    /// the orchestration's real dir + marker writes are exercised, and
    /// returns a canned count — no `dnf`/`createrepo_c` needed.
    struct MockRunner {
        fail_reposync: bool,
        fail_createrepo: bool,
    }
    impl MirrorSyncRunner for MockRunner {
        fn reposync(&self, _name: &str, _upstream: &str, dest: &Path) -> Result<u32, String> {
            if self.fail_reposync {
                return Err("network unreachable".into());
            }
            std::fs::write(dest.join("pkg-a-1.0.rpm"), b"rpm").unwrap();
            std::fs::write(dest.join("pkg-b-2.0.rpm"), b"rpm").unwrap();
            Ok(count_rpms(dest))
        }
        fn createrepo(&self, dir: &Path) -> Result<(), String> {
            if self.fail_createrepo {
                return Err("createrepo_c not found".into());
            }
            std::fs::create_dir_all(dir.join("repodata")).unwrap();
            Ok(())
        }
    }

    #[test]
    fn sync_mirror_pulls_indexes_and_stamps_last_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let m = &core_pack()[0];
        let runner = MockRunner {
            fail_reposync: false,
            fail_createrepo: false,
        };
        let report = sync_mirror(&runner, m, tmp.path(), 1_700_000_000_000).expect("sync");
        assert_eq!(report.name, "magic-mesh");
        assert_eq!(report.rpm_count, 2);
        assert_eq!(report.synced_at_ms, 1_700_000_000_000);
        assert_eq!(report.served_baseurl, m.file_baseurl(tmp.path()));
        // The marker is on disk AND readable back through the model.
        assert_eq!(m.last_sync_ms(tmp.path()), Some(1_700_000_000_000));
        assert!(m.local_dir(tmp.path()).join("repodata").exists());
    }

    #[test]
    fn sync_mirror_refuses_a_disabled_mirror_before_any_work() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = core_pack()[0].clone();
        m.enabled = false;
        let runner = MockRunner {
            fail_reposync: false,
            fail_createrepo: false,
        };
        let r = sync_mirror(&runner, &m, tmp.path(), 1);
        assert_eq!(r, Err(MirrorSyncError::Disabled));
        // No dir, no marker — the refusal short-circuits before mkdir.
        assert!(m.last_sync_ms(tmp.path()).is_none());
    }

    #[test]
    fn sync_mirror_does_not_stamp_when_reposync_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let m = &core_pack()[0];
        let runner = MockRunner {
            fail_reposync: true,
            fail_createrepo: false,
        };
        let r = sync_mirror(&runner, m, tmp.path(), 1);
        assert!(matches!(r, Err(MirrorSyncError::Reposync(_))));
        // Freshness is stamped LAST — a failed pull leaves no .last-sync,
        // so the mirror never falsely reports as synced.
        assert!(m.last_sync_ms(tmp.path()).is_none());
    }

    #[test]
    fn sync_mirror_does_not_stamp_when_createrepo_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let m = &core_pack()[0];
        let runner = MockRunner {
            fail_reposync: false,
            fail_createrepo: true,
        };
        let r = sync_mirror(&runner, m, tmp.path(), 1);
        assert!(matches!(r, Err(MirrorSyncError::Createrepo(_))));
        assert!(m.last_sync_ms(tmp.path()).is_none());
    }

    // ---- W62 dnf .repo rewrite ----------------------------------

    #[test]
    fn render_dnf_repo_puts_local_first_upstream_fallback() {
        let m = &core_pack()[0];
        let root = Path::new("/wg");
        let repo = render_dnf_repo(m, root);
        // Section id + both baseurls present, local BEFORE upstream so dnf
        // self-serves and only falls back to GitHub.
        assert!(repo.contains("[magic-mesh]"));
        let local = "file:///wg/mirrors/magic-mesh";
        let up = "matthewmackes.github.io";
        let li = repo.find(local).expect("local baseurl present");
        let ui = repo.find(up).expect("upstream fallback present");
        assert!(li < ui, "local file:// must precede the upstream fallback");
        assert!(repo.contains("enabled=1"));
    }

    #[test]
    fn render_dnf_repo_marks_disabled_mirror_enabled_zero() {
        let mut m = core_pack()[0].clone();
        m.enabled = false;
        let repo = render_dnf_repo(&m, Path::new("/wg"));
        assert!(repo.contains("enabled=0"));
    }

    #[test]
    fn write_dnf_repo_lands_a_named_file_with_the_rendered_body() {
        let tmp = tempfile::tempdir().unwrap();
        let m = &core_pack()[0];
        let repo_dir = tmp.path().join("yum.repos.d");
        let path = write_dnf_repo(m, tmp.path(), &repo_dir).expect("write");
        assert_eq!(path, repo_dir.join("mackes-mirror-magic-mesh.repo"));
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, render_dnf_repo(m, tmp.path()));
    }
}
