//! FPG-2 — the Syncthing-replicated revision log.
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
//! node ([`crate::elect_revision`]). Reads admit only canonical zero-padded
//! filenames, regular non-symlink files, and revisions passing
//! [`Revision::validate`].

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    revision.validate().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing invalid revision: {e}"),
        )
    })?;
    std::fs::create_dir_all(dir)?;
    let path = revision_path(dir, revision.version);
    let yaml = revision
        .to_yaml()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    // A unique hidden staging file plus hard-link creation gives us an
    // atomic, no-replace install on the replicated filesystem. `rename`
    // replaces an existing destination on Unix, which would violate the
    // append-only contract under a concurrent mint or a hostile symlink.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = dir.join(format!(
        ".{:020}.{}.{}.yaml.tmp",
        revision.version,
        std::process::id(),
        stamp
    ));
    let result: io::Result<()> = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(yaml.as_bytes())?;
        file.sync_all()?;
        std::fs::hard_link(&tmp, &path).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "revision {} already in the log (append-only)",
                        revision.version
                    ),
                )
            } else {
                e
            }
        })?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    result?;
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
        .filter_map(|e| {
            let path = e.path();
            let file_name = path.file_name()?.to_str()?;
            let stem = file_name.strip_suffix(".yaml")?;
            if stem.len() != 20 || !stem.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let version = stem.parse::<u64>().ok()?;
            if revision_path(dir, version) != path {
                return None;
            }
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if !metadata.file_type().is_file() {
                return None;
            }
            let raw = std::fs::read_to_string(&path).ok()?;
            let revision = Revision::from_yaml(&raw).ok()?;
            if revision.version != version || revision.validate().is_err() {
                return None;
            }
            Some(revision)
        })
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

// ── FPG-5: apply-acks ───────────────────────────────────────────────
//
// After a node converges to a revision it writes an ack at
// `<root>/fleet/acks/<version>/<hostname>.json`; replication gossips
// it back to every node (incl. the author, whose FSM advances to
// Verified when acks arrive — Q14). Own-file authority, the PEERVER
// pattern: each node only ever writes its own ack file.

/// One node's apply outcome for one revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApplyAck {
    /// Acking node's hostname.
    pub peer: String,
    /// `"applied"` or `"failed"` (plus anything richer later — read
    /// tolerantly).
    pub status: String,
    /// Ack time, Unix seconds.
    pub at: u64,
    /// Optional failure detail.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// The acks directory for one revision version.
#[must_use]
pub fn acks_dir(workgroup_root: &Path, version: u64) -> PathBuf {
    workgroup_root
        .join("fleet")
        .join("acks")
        .join(format!("{version:020}"))
}

/// Write this node's ack for `version` (atomic temp + rename;
/// overwrites its own prior ack — re-applies re-ack).
///
/// # Errors
/// IO or serialization failures.
pub fn write_ack(workgroup_root: &Path, version: u64, ack: &ApplyAck) -> io::Result<PathBuf> {
    let dir = acks_dir(workgroup_root, version);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", ack.peer));
    let json = serde_json::to_string_pretty(ack)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let tmp = dir.join(format!(".{}.json.tmp", ack.peer));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every parseable ack for `version`, sorted by peer. Tolerant
/// of junk/half-replicated files, like [`read_revisions`].
#[must_use]
pub fn read_acks(workgroup_root: &Path, version: u64) -> Vec<ApplyAck> {
    let dir = acks_dir(workgroup_root, version);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<ApplyAck> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.peer.cmp(&b.peer));
    out
}

// ── PD-9: reconcile nudges ──────────────────────────────────────────
//
// "Apply now" writes `<root>/fleet/nudges/<hostname>`; replication
// carries it to the target, whose reconcile worker consumes the file
// and converges immediately instead of waiting out its cadence. The
// nudge only hurries convergence to the existing elected head — it
// can never fork per-peer state (Q16).

/// The nudges directory.
#[must_use]
pub fn nudges_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("fleet").join("nudges")
}

/// Write a nudge for `hostname` (idempotent — re-nudging while one
/// is pending is a no-op).
///
/// # Errors
/// IO failures.
pub fn write_nudge(workgroup_root: &Path, hostname: &str) -> io::Result<PathBuf> {
    write_nudge_payload(workgroup_root, hostname, "reconcile\n")
}

/// Write a nudge carrying an authenticated producer envelope.
///
/// The marker is still a one-file, idempotent trigger, but the payload lets
/// the destination worker verify the exact Bus capability before it starts a
/// reconcile. Legacy callers may use [`write_nudge`] and will be rejected by
/// an authenticated consumer until they are migrated.
pub fn write_nudge_payload(
    workgroup_root: &Path,
    hostname: &str,
    payload: &str,
) -> io::Result<PathBuf> {
    let dir = nudges_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(hostname);
    std::fs::write(&path, payload)?;
    Ok(path)
}

/// Consume the destination nudge and return its producer envelope.
#[must_use]
pub fn take_nudge_payload(workgroup_root: &Path, hostname: &str) -> Option<String> {
    let path = nudges_dir(workgroup_root).join(hostname);
    let payload = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(payload)
}

/// Consume this host's pending nudge — `true` exactly once per nudge
/// (the file is removed).
#[must_use]
pub fn take_nudge(workgroup_root: &Path, hostname: &str) -> bool {
    take_nudge_payload(workgroup_root, hostname).is_some()
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
    fn only_canonical_valid_revision_files_are_electable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        write_revision(&dir, &rev(1, 100, "peer:pine")).unwrap();
        let yaml = rev(9, 900, "peer:oak").to_yaml().unwrap();
        std::fs::write(dir.join("9.yaml"), &yaml).unwrap();
        std::fs::write(
            dir.join("00000000000000000002.yaml"),
            rev(0, 0, "peer:oak").to_yaml().unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("00000000000000000003.yaml"),
            rev(3, 300, "peer/evil").to_yaml().unwrap(),
        )
        .unwrap();
        let all = read_revisions(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, 1);
    }

    #[test]
    fn invalid_revision_is_rejected_before_any_file_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        let error = write_revision(&dir, &rev(0, 100, "peer:pine")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!dir.exists(), "invalid input must not create the log");
    }

    #[cfg(unix)]
    #[test]
    fn append_install_does_not_follow_a_destination_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = revisions_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        let outside = tmp.path().join("outside.yaml");
        std::fs::write(&outside, "untouched").unwrap();
        std::os::unix::fs::symlink(&outside, revision_path(&dir, 1)).unwrap();
        let error = write_revision(&dir, &rev(1, 100, "peer:pine")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    }

    #[test]
    fn acks_round_trip_and_reack_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let ack = ApplyAck {
            peer: "pine".into(),
            status: "applied".into(),
            at: 100,
            detail: String::new(),
        };
        write_ack(tmp.path(), 3, &ack).unwrap();
        write_ack(
            tmp.path(),
            3,
            &ApplyAck {
                peer: "oak".into(),
                status: "failed".into(),
                at: 110,
                detail: "dnf exploded".into(),
            },
        )
        .unwrap();
        let acks = read_acks(tmp.path(), 3);
        assert_eq!(acks.len(), 2);
        assert_eq!(acks[0].peer, "oak");
        assert_eq!(acks[0].detail, "dnf exploded");
        // Re-ack overwrites own file (re-apply -> re-ack).
        write_ack(
            tmp.path(),
            3,
            &ApplyAck {
                peer: "oak".into(),
                status: "applied".into(),
                at: 120,
                detail: String::new(),
            },
        )
        .unwrap();
        let again = read_acks(tmp.path(), 3);
        assert_eq!(again.len(), 2);
        assert_eq!(again[0].status, "applied");
    }

    #[test]
    fn nudges_are_consumed_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!take_nudge(tmp.path(), "pine"), "no nudge yet");
        write_nudge(tmp.path(), "pine").unwrap();
        write_nudge(tmp.path(), "pine").unwrap(); // idempotent re-nudge
        assert!(take_nudge(tmp.path(), "pine"), "consumed");
        assert!(!take_nudge(tmp.path(), "pine"), "exactly once");
    }

    #[test]
    fn acks_for_unacked_version_are_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_acks(tmp.path(), 99).is_empty());
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
