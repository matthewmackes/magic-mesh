//! FPG-2 — the LizardFS revision log.
//!
//! Revisions live as individual YAML files in
//! `<workgroup-root>/fleet/revisions/<version>.yaml` on the
//! replicated volume: **replication is the gossip transport** and the
//! directory is the authoritative, append-only log (no SQL truth —
//! any SQLite copy is a per-node read mirror). Filenames zero-pad the
//! `u64` version to 20 digits so lexical order == numeric order.
//!
//! Every node can mint (leaderless, FPG-3): `next_version` derives
//! from the highest version present, and [`write_revision`] is
//! append-only — an existing version file is never overwritten, so a
//! mint race degrades to two distinct files whose election the
//! `version → at → author` total order settles identically on every
//! node ([`crate::elect_revision`]).

use std::io;
use std::path::{Path, PathBuf};

use crate::Revision;

/// The revision-log directory under the replicated workgroup root.
#[must_use]
pub fn revisions_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("fleet").join("revisions")
}

/// The canonical filename for a revision version (zero-padded so
/// lexical directory order matches numeric order).
#[must_use]
pub fn revision_path(dir: &Path, version: u64) -> PathBuf {
    dir.join(format!("{version:020}.yaml"))
}

/// Highest version present in the log + 1 (1 for an empty/missing
/// log). What a minting node stamps on its next revision.
#[must_use]
pub fn next_version(dir: &Path) -> u64 {
    read_revisions(dir)
        .iter()
        .map(|r| r.version)
        .max()
        .map_or(1, |v| v.saturating_add(1))
}

/// Append a revision to the log (atomic temp + rename; creates the
/// directory). **Append-only:** refuses to replace an existing file
/// for the same version — history is immutable; rollback mints a
/// higher version carrying the old spec (FPG-4 / Q6).
///
/// # Errors
/// IO failures, serialization failure, or the version already existing.
pub fn write_revision(dir: &Path, revision: &Revision) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = revision_path(dir, revision.version);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "revision {} already in the log (append-only)",
                revision.version
            ),
        ));
    }
    let yaml = revision
        .to_yaml()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let tmp = dir.join(format!(".{:020}.yaml.tmp", revision.version));
    std::fs::write(&tmp, yaml)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every parseable revision in the log, sorted ascending by
/// version. Tolerant: unparsable / foreign files are skipped (a
/// half-replicated write from a peer must not poison the log read) —
/// the next replication pass completes them.
#[must_use]
pub fn read_revisions(dir: &Path) -> Vec<Revision> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Revision> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| Revision::from_yaml(&raw).ok())
        .collect();
    out.sort_by_key(|r| (r.version, r.at, r.author.clone()));
    out
}

/// The elected head of the log — the revision every node converges
/// to (FPG-3/FPG-6: a cold or partitioned node applies this
/// immediately; history back-fills by replication on its own time).
#[must_use]
pub fn elect_head(dir: &Path) -> Option<Revision> {
    let all = read_revisions(dir);
    crate::elect_revision(&all).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BaselineSpec;

    fn rev(version: u64, at: u64, author: &str) -> Revision {
        Revision {
            version,
            author: author.to_string(),
            at,
            spec: BaselineSpec::default(),
        }
    }

    #[test]
    fn round_trips_through_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        write_revision(&dir, &rev(1, 100, "peer:pine")).unwrap();
        write_revision(&dir, &rev(2, 200, "peer:oak")).unwrap();
        let all = read_revisions(&dir);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].version, 1);
        assert_eq!(all[1].author, "peer:oak");
    }

    #[test]
    fn log_is_append_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        write_revision(&dir, &rev(7, 100, "peer:pine")).unwrap();
        let again = write_revision(&dir, &rev(7, 999, "peer:oak"));
        assert!(again.is_err(), "same version must never be replaced");
        assert_eq!(read_revisions(&dir)[0].at, 100, "original survives");
    }

    #[test]
    fn next_version_is_max_plus_one_and_one_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        assert_eq!(next_version(&dir), 1, "empty/missing log starts at 1");
        write_revision(&dir, &rev(41, 100, "peer:pine")).unwrap();
        assert_eq!(next_version(&dir), 42);
    }

    #[test]
    fn elect_head_picks_the_newest_wins_winner() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        write_revision(&dir, &rev(3, 100, "peer:pine")).unwrap();
        write_revision(&dir, &rev(5, 50, "peer:oak")).unwrap();
        assert_eq!(elect_head(&dir).unwrap().version, 5);
    }

    #[test]
    fn junk_files_do_not_poison_the_read() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        write_revision(&dir, &rev(1, 100, "peer:pine")).unwrap();
        std::fs::write(dir.join("garbage.yaml"), "{{not yaml").unwrap();
        std::fs::write(dir.join("README.txt"), "hello").unwrap();
        assert_eq!(read_revisions(&dir).len(), 1);
    }

    #[test]
    fn settings_domain_round_trips_in_the_log() {
        // FPG-1 / Q9 — settings fold into the baseline wire format.
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        let mut r = rev(1, 100, "peer:pine");
        r.spec
            .settings
            .insert("theme.accent".into(), "\"#0f62fe\"".into());
        write_revision(&dir, &r).unwrap();
        let back = elect_head(&dir).unwrap();
        assert_eq!(back.spec.settings["theme.accent"], "\"#0f62fe\"");
    }
}
