//! QC-3 (CONSTRUCT-CLOUD) — the Kolla image airgap lane.
//!
//! Operator-mirrored archives on the mesh share, checksum-verified, then
//! `podman load`ed locally. **No registry is ever contacted** (design
//! Q18/Q63 — the fleet is airgapped; a pull is a doomed operation and a
//! design violation).
//!
//! ## The decided archive layout (QC-3 — the runbook §4 deferred this here)
//!
//! ```text
//! <mesh-share>/kolla/<release>/<image-basename>-<release>.tar
//! <mesh-share>/kolla/<release>/SHA256SUMS
//! ```
//!
//! Concretely, for the pinned release `2024.1` on the canonical share:
//!
//! ```text
//! /mnt/mesh-storage/kolla/2024.1/nova-api-2024.1.tar
//! /mnt/mesh-storage/kolla/2024.1/mariadb-server-2024.1.tar
//! /mnt/mesh-storage/kolla/2024.1/SHA256SUMS
//! ```
//!
//! - `<image-basename>` is [`ServiceKind::image_name`] (dashed, the Kolla
//!   convention); the whole filename is [`ServiceKind::archive_file_name`].
//! - The release is in **both** the directory and the filename so a stray
//!   archive can never be loaded for the wrong pin, and two releases can
//!   coexist on the share during an upgrade (runbook §10: an upgrade is a new
//!   mirrored archive set + a fleet-state pin change).
//! - `SHA256SUMS` is plain `sha256sum` output (`<64-hex>  <filename>`, one
//!   line per archive) generated **in** the release directory:
//!   `cd kolla/<release> && sha256sum *.tar > SHA256SUMS`.
//!
//! ## The operator mirror procedure this lane consumes (runbook §4)
//!
//! On a connected host: `podman pull` the pinned images, then
//! `podman save -o <image-basename>-<release>.tar
//! quay.io/openstack.kolla/<image-basename>:<release>` — the default
//! docker-archive format **embeds the source tag**, so a later `podman load`
//! restores exactly the [`ServiceKind::image_ref`] the worker gates on. Write
//! `SHA256SUMS`, drop the directory under `<share>/kolla/<release>/`, and
//! Syncthing replicates it to every node.
//!
//! ## The trust boundary (§7 honesty + supply-chain)
//!
//! The share is writable by every mesh node; the checksum file is the
//! operator's pinned statement of what the archives must be. An archive is
//! loaded **only** after its `SHA256` matches its `SHA256SUMS` entry —
//! a missing sums file / missing entry / mismatched digest all refuse the
//! load with a typed [`ArchiveStatus`], and a mismatch additionally rides
//! the `[!]` alert lane (a corrupt or tampered archive must never enter the
//! image store). "Loading" is transient within a converge tick (the load is
//! synchronous); a failed load surfaces as a `Failed` mirror row naming the
//! archive.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::catalog::ServiceKind;

/// The share-relative directory the archive lane lives under:
/// `<share>/kolla/<release>/…`.
pub const KOLLA_ARCHIVE_DIR: &str = "kolla";

/// The checksum manifest beside the archives — plain `sha256sum` output.
pub const SHA256SUMS_FILE: &str = "SHA256SUMS";

/// The release's archive directory: `<share>/kolla/<release>`.
#[must_use]
pub fn kolla_release_dir(share_root: &Path, release: &str) -> PathBuf {
    share_root.join(KOLLA_ARCHIVE_DIR).join(release)
}

/// The full expected archive path for `kind` at `release`:
/// `<share>/kolla/<release>/<image-basename>-<release>.tar`.
#[must_use]
pub fn archive_path(share_root: &Path, kind: ServiceKind, release: &str) -> PathBuf {
    kolla_release_dir(share_root, release).join(kind.archive_file_name(release))
}

/// The release's checksum manifest path: `<share>/kolla/<release>/SHA256SUMS`.
#[must_use]
pub fn sums_path(share_root: &Path, release: &str) -> PathBuf {
    kolla_release_dir(share_root, release).join(SHA256SUMS_FILE)
}

/// Parse `sha256sum`-format text into `filename → lowercase hex digest`.
///
/// Accepts the standard text-mode form (`<hex>  <name>`), the binary-mode
/// marker (`<hex> *<name>`), and a `./` name prefix; blank lines, `#`
/// comments, and malformed lines (non-64-hex digest, empty name) are
/// skipped — an unparseable entry can only ever make verification *fail*,
/// never pass.
#[must_use]
pub fn parse_sha256sums(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((digest, rest)) = line.split_once(|c: char| c.is_whitespace()) else {
            continue;
        };
        let name = rest.trim_start().trim_start_matches('*');
        let name = name.strip_prefix("./").unwrap_or(name);
        if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty() {
            map.insert(name.to_string(), digest.to_ascii_lowercase());
        }
    }
    map
}

/// Stream-hash a file to its lowercase-hex `SHA256` (64 KiB chunks — Kolla
/// archives run to a GiB+, so the whole file is never held in memory).
///
/// # Errors
/// Any I/O failure opening or reading `path`.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// The typed outcome of checking the share for one service's archive.
///
/// Exactly one variant per honest sub-state the mirror distinguishes.
/// Only [`Self::Verified`] may be `podman load`ed (the trust boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveStatus {
    /// The archive exists and its `SHA256` matches its `SHA256SUMS` entry —
    /// the only state a load is permitted from.
    Verified {
        /// The verified archive.
        path: PathBuf,
        /// Its (matching) lowercase-hex digest.
        sha256: String,
    },
    /// The share root itself is absent — the Syncthing volume isn't
    /// mounted/provisioned on this node yet. Gated, no alert.
    ShareAbsent {
        /// The missing share root.
        share_root: PathBuf,
        /// The archive path the service is waiting for.
        wanted: PathBuf,
    },
    /// The share is up but the archive (or its whole `kolla/<release>/`
    /// directory) hasn't arrived — awaiting the operator mirror + Syncthing
    /// replication. Gated, no alert.
    ArchiveMissing {
        /// The exact archive path the service is waiting for.
        wanted: PathBuf,
    },
    /// The archive is present but cannot be *verified* — the `SHA256SUMS`
    /// file is absent or has no entry for it. Never loaded. Gated, no alert
    /// (an in-flight Syncthing replication commonly lands the tar before the
    /// sums file).
    SumsUnavailable {
        /// The present-but-unverifiable archive.
        archive: PathBuf,
        /// The expected checksum-manifest path.
        sums: PathBuf,
        /// Which half is missing (the file, or its entry).
        detail: String,
    },
    /// The archive hashes to something other than its `SHA256SUMS` entry —
    /// corrupt or tampered. **Never loaded**; Gated + the `[!]` alert lane.
    ChecksumMismatch {
        /// The failing archive.
        path: PathBuf,
        /// The digest `SHA256SUMS` pins.
        expected: String,
        /// The digest the file actually hashes to.
        actual: String,
    },
    /// Reading the sums file / hashing the archive failed for a concrete
    /// I/O reason (permissions, disk error, a file vanishing mid-read).
    ReadFailed {
        /// The path that failed.
        path: PathBuf,
        /// The I/O detail.
        reason: String,
    },
}

/// Check the share for `kind`'s archive at the pinned `release` and verify
/// it against `SHA256SUMS`.
///
/// The read-only half of the QC-3 lane (the `podman load` half lives on
/// the [`super::podman::PodmanRunner`] seam).
///
/// The hash runs only when the caller already knows the image is absent
/// locally, so steady-state ticks never re-hash; a persistent
/// [`ArchiveStatus::ChecksumMismatch`] re-hashes each tick until the
/// operator re-mirrors — deliberate, so the mirror row heals itself the
/// moment a good archive replicates in.
#[must_use]
pub fn check_archive(share_root: &Path, kind: ServiceKind, release: &str) -> ArchiveStatus {
    let wanted = archive_path(share_root, kind, release);
    if !share_root.is_dir() {
        return ArchiveStatus::ShareAbsent {
            share_root: share_root.to_path_buf(),
            wanted,
        };
    }
    if !wanted.is_file() {
        return ArchiveStatus::ArchiveMissing { wanted };
    }
    let sums = sums_path(share_root, release);
    let text = match std::fs::read_to_string(&sums) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ArchiveStatus::SumsUnavailable {
                archive: wanted,
                sums,
                detail: format!("{SHA256SUMS_FILE} is absent beside it"),
            };
        }
        Err(e) => {
            return ArchiveStatus::ReadFailed {
                path: sums,
                reason: e.to_string(),
            };
        }
    };
    let entry = kind.archive_file_name(release);
    let Some(expected) = parse_sha256sums(&text).get(&entry).cloned() else {
        return ArchiveStatus::SumsUnavailable {
            archive: wanted,
            sums,
            detail: format!("{SHA256SUMS_FILE} has no entry for {entry}"),
        };
    };
    let actual = match sha256_file(&wanted) {
        Ok(hex) => hex,
        Err(e) => {
            return ArchiveStatus::ReadFailed {
                path: wanted,
                reason: e.to_string(),
            };
        }
    };
    if actual == expected {
        ArchiveStatus::Verified {
            path: wanted,
            sha256: actual,
        }
    } else {
        ArchiveStatus::ChecksumMismatch {
            path: wanted,
            expected,
            actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `content` as `kind`'s archive under a share layout, returning
    /// the archive path. `sums` controls the manifest beside it.
    fn seed_archive(
        share: &Path,
        kind: ServiceKind,
        release: &str,
        content: &[u8],
        sums: Option<&str>,
    ) -> PathBuf {
        let dir = kolla_release_dir(share, release);
        std::fs::create_dir_all(&dir).unwrap();
        let path = archive_path(share, kind, release);
        std::fs::write(&path, content).unwrap();
        if let Some(text) = sums {
            std::fs::write(dir.join(SHA256SUMS_FILE), text).unwrap();
        }
        path
    }

    #[test]
    fn layout_is_the_documented_shape() {
        // The decided QC-3 layout — pinned so the runbook's exact paths
        // can't silently drift.
        let share = Path::new("/mnt/mesh-storage");
        assert_eq!(
            archive_path(share, ServiceKind::NovaApi, "2024.1"),
            PathBuf::from("/mnt/mesh-storage/kolla/2024.1/nova-api-2024.1.tar")
        );
        assert_eq!(
            archive_path(share, ServiceKind::Mariadb, "2024.1"),
            PathBuf::from("/mnt/mesh-storage/kolla/2024.1/mariadb-server-2024.1.tar")
        );
        assert_eq!(
            sums_path(share, "2024.1"),
            PathBuf::from("/mnt/mesh-storage/kolla/2024.1/SHA256SUMS")
        );
    }

    #[test]
    fn sha256_matches_the_known_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            sha256_file(&empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let abc = dir.path().join("abc");
        std::fs::write(&abc, b"abc").unwrap();
        assert_eq!(
            sha256_file(&abc).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sums_parser_accepts_the_sha256sum_dialects() {
        let hex_a = "a".repeat(64);
        let hex_b = "B".repeat(64); // uppercase digests normalize down
        let text = format!(
            "{hex_a}  keystone-2024.1.tar\n\
             {hex_b} *nova-api-2024.1.tar\n\
             {hex_a}  ./cinder-api-2024.1.tar\n\
             # a comment\n\
             \n\
             not-a-digest  garbage.tar\n\
             {hex_a}\n"
        );
        let map = parse_sha256sums(&text);
        assert_eq!(map.get("keystone-2024.1.tar"), Some(&hex_a));
        assert_eq!(map.get("nova-api-2024.1.tar"), Some(&"b".repeat(64)));
        assert_eq!(map.get("cinder-api-2024.1.tar"), Some(&hex_a));
        assert_eq!(map.len(), 3, "malformed lines are skipped: {map:?}");
    }

    #[test]
    fn share_absent_names_both_the_root_and_the_wanted_archive() {
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("not-mounted");
        let st = check_archive(&share, ServiceKind::Keystone, "2024.1");
        let ArchiveStatus::ShareAbsent { share_root, wanted } = st else {
            unreachable!("wrong status: {st:?}");
        };
        assert_eq!(share_root, share);
        assert!(wanted.ends_with("kolla/2024.1/keystone-2024.1.tar"));
    }

    #[test]
    fn missing_archive_names_the_exact_wanted_path() {
        let dir = tempfile::tempdir().unwrap();
        // Share up, kolla dir never mirrored — the commonest awaiting state.
        let st = check_archive(dir.path(), ServiceKind::NovaApi, "2024.1");
        let ArchiveStatus::ArchiveMissing { wanted } = st else {
            unreachable!("wrong status: {st:?}");
        };
        assert_eq!(
            wanted,
            archive_path(dir.path(), ServiceKind::NovaApi, "2024.1")
        );
    }

    #[test]
    fn archive_without_sums_is_unverifiable_not_loadable() {
        let dir = tempfile::tempdir().unwrap();
        seed_archive(dir.path(), ServiceKind::Keystone, "2024.1", b"bytes", None);
        let st = check_archive(dir.path(), ServiceKind::Keystone, "2024.1");
        assert!(
            matches!(&st, ArchiveStatus::SumsUnavailable { detail, .. } if detail.contains("absent")),
            "{st:?}"
        );
        // Sums present but for a different archive → still unverifiable.
        let other = format!("{}  nova-api-2024.1.tar\n", "a".repeat(64));
        seed_archive(
            dir.path(),
            ServiceKind::Keystone,
            "2024.1",
            b"bytes",
            Some(&other),
        );
        let st = check_archive(dir.path(), ServiceKind::Keystone, "2024.1");
        assert!(
            matches!(&st, ArchiveStatus::SumsUnavailable { detail, .. } if detail.contains("no entry")),
            "{st:?}"
        );
    }

    #[test]
    fn mismatch_and_verified_are_decided_by_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"kolla image bytes";
        // Wrong pin → mismatch, with both digests carried for the operator.
        let bad = format!("{}  keystone-2024.1.tar\n", "0".repeat(64));
        seed_archive(
            dir.path(),
            ServiceKind::Keystone,
            "2024.1",
            content,
            Some(&bad),
        );
        let st = check_archive(dir.path(), ServiceKind::Keystone, "2024.1");
        let ArchiveStatus::ChecksumMismatch {
            expected, actual, ..
        } = st
        else {
            unreachable!("wrong status: {st:?}");
        };
        assert_eq!(expected, "0".repeat(64));
        assert_ne!(actual, expected);
        // Right pin → verified.
        let path = archive_path(dir.path(), ServiceKind::Keystone, "2024.1");
        let good = format!("{}  keystone-2024.1.tar\n", sha256_file(&path).unwrap());
        seed_archive(
            dir.path(),
            ServiceKind::Keystone,
            "2024.1",
            content,
            Some(&good),
        );
        let st = check_archive(dir.path(), ServiceKind::Keystone, "2024.1");
        assert!(
            matches!(&st, ArchiveStatus::Verified { path: p, .. } if *p == path),
            "{st:?}"
        );
    }
}
