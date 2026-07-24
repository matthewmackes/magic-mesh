//! PLANES-22 — the image catalog (W53/W54/W55).
//!
//! The mesh builds four kinds of image (W53): the install **ISO** (+
//! kickstart), the **VM** golden image, **container** images, and the
//! **USB** writer image. Each build is a job on an execution-tagged node
//! (W54), and its output lands as a versioned dir with a TOML manifest on
//! the Syncthing share (`<root>/images/<name>/<version>/manifest.toml`, W55).
//!
//! This is the pure core: the four kinds are a fixed vocabulary always
//! shown (so the catalog lists what *can* be built even before any
//! build), and `load_manifests` walks the versioned dirs for what *has*
//! been built. The `mackesd images` CLI verb + the Provisioning ▸ Images
//! panel render on top.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The four image kinds the mesh can build (W53).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageKind {
    /// Install ISO with embedded kickstart; used for bare-metal provisioning.
    Iso,
    /// Pre-enrolled golden VM image for libvirt/KVM fleet provisioning.
    Vm,
    /// OCI container images of the mesh services.
    Container,
    /// Bootable USB writer image (dd-able); shares content with the ISO build.
    Usb,
}

impl ImageKind {
    /// Stable wire token (also the manifest `kind` field).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ImageKind::Iso => "iso",
            ImageKind::Vm => "vm",
            ImageKind::Container => "container",
            ImageKind::Usb => "usb",
        }
    }

    /// Sentence-case label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ImageKind::Iso => "Install ISO",
            ImageKind::Vm => "VM golden image",
            ImageKind::Container => "Container image",
            ImageKind::Usb => "USB writer",
        }
    }

    /// One-line description of what the build produces.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            ImageKind::Iso => "Magic-on-Cosmic install ISO + kickstart, boot-menu profile choice",
            ImageKind::Vm => "Pre-enrolled golden VM image for libvirt/KVM provisioning",
            ImageKind::Container => "OCI container images of the mesh services",
            ImageKind::Usb => "Bootable USB writer image (dd-able), USB/ISO only (PXE deferred)",
        }
    }

    /// The fixed vocabulary, display order.
    #[must_use]
    pub const fn all() -> [ImageKind; 4] {
        [
            ImageKind::Iso,
            ImageKind::Vm,
            ImageKind::Container,
            ImageKind::Usb,
        ]
    }

    /// Parse a manifest `kind` token.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::all().into_iter().find(|k| k.as_str() == s)
    }
}

/// One built image's manifest (the TOML in a versioned dir, W55).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageManifest {
    /// Image name (the first dir level under `images/`).
    pub name: String,
    /// One of the [`ImageKind`] tokens.
    pub kind: String,
    /// Version string (the second dir level).
    pub version: String,
    /// Build completion time (Unix ms), if recorded.
    #[serde(default)]
    pub built_at_ms: Option<u64>,
    /// Output size in bytes, if recorded.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// The install profile this image bakes in, if any (links to PLANES-21).
    #[serde(default)]
    pub profile: Option<String>,
}

/// The images directory (`<root>/images/`).
#[must_use]
pub fn images_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("images")
}

/// Walk `<root>/images/<name>/<version>/manifest.toml` (junk-tolerant)
/// and return every built manifest, newest first by build time.
#[must_use]
pub fn load_manifests(workgroup_root: &Path) -> Vec<ImageManifest> {
    let mut out = Vec::new();
    let Ok(names) = std::fs::read_dir(images_dir(workgroup_root)) else {
        return out;
    };
    for name_entry in names.filter_map(Result::ok) {
        let Ok(name_type) = name_entry.file_type() else {
            continue;
        };
        if !name_type.is_dir() || name_type.is_symlink() {
            continue;
        }
        let name = name_entry.file_name();
        let Ok(versions) = std::fs::read_dir(name_entry.path()) else {
            continue;
        };
        for ver_entry in versions.filter_map(Result::ok) {
            let Ok(version_type) = ver_entry.file_type() else {
                continue;
            };
            if !version_type.is_dir() || version_type.is_symlink() {
                continue;
            }
            let manifest = ver_entry.path().join("manifest.toml");
            if let Ok(raw) = std::fs::read_to_string(&manifest) {
                if let Ok(m) = toml::from_str::<ImageManifest>(&raw) {
                    // The replicated path is part of the identity. Do not
                    // surface a valid TOML blob copied under another image or
                    // version directory; that would let stale/hostile state
                    // masquerade as a different build.
                    let path_name = name.to_string_lossy().into_owned();
                    let path_version = ver_entry.file_name().to_string_lossy().into_owned();
                    if validate_manifest(&m).is_ok()
                        && m.name == path_name
                        && m.version == path_version
                    {
                        out.push(m);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.built_at_ms.cmp(&a.built_at_ms));
    out
}

// ─────────────────────────────────────────────────────────────────
// W55 — register a completed build. A build job (W54) calls
// record_manifest when its output lands, writing the versioned-dir TOML
// that load_manifests + the Images panel then surface. Syncthing
// replicates it so the whole fleet sees the new build.
// ─────────────────────────────────────────────────────────────────

/// Why recording a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestWriteError {
    /// `name` was empty or not a path-safe `[a-z0-9._-]+` token.
    BadName(String),
    /// `version` was empty or not a path-safe `[a-z0-9._-]+` token.
    BadVersion(String),
    /// `kind` is not one of the four [`ImageKind`] tokens.
    BadKind(String),
    /// TOML serialization failed (practically never).
    Serialize(String),
    /// Filesystem write failed.
    Io(String),
}

impl std::fmt::Display for ManifestWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName(n) => write!(f, "invalid image name '{n}' (need a [a-z0-9._-]+ token)"),
            Self::BadVersion(v) => write!(f, "invalid version '{v}' (need a [a-z0-9._-]+ token)"),
            Self::BadKind(k) => {
                write!(f, "invalid kind '{k}' (expected iso|vm|container|usb)")
            }
            Self::Serialize(e) => write!(f, "serialize manifest: {e}"),
            Self::Io(e) => write!(f, "write manifest: {e}"),
        }
    }
}
impl std::error::Error for ManifestWriteError {}

/// A name/version is a path-safe token — each becomes a directory level.
fn is_path_safe(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_'
        })
}

/// Validate a manifest's kind + name + version (the build job's inputs)
/// without writing it.
///
/// # Errors
/// [`ManifestWriteError`] naming the offending field.
pub fn validate_manifest(m: &ImageManifest) -> Result<(), ManifestWriteError> {
    if !is_path_safe(&m.name) {
        return Err(ManifestWriteError::BadName(m.name.clone()));
    }
    if !is_path_safe(&m.version) {
        return Err(ManifestWriteError::BadVersion(m.version.clone()));
    }
    if ImageKind::parse(&m.kind).is_none() {
        return Err(ManifestWriteError::BadKind(m.kind.clone()));
    }
    Ok(())
}

/// Write `manifest` to `<root>/images/<name>/<version>/manifest.toml`
/// after validating it, overwriting an existing manifest at that version.
/// Returns the path written.
///
/// # Errors
/// [`ManifestWriteError`] on validation, serialization, or IO failure.
pub fn record_manifest(
    manifest: &ImageManifest,
    workgroup_root: &Path,
) -> Result<PathBuf, ManifestWriteError> {
    validate_manifest(manifest)?;
    let dir = images_dir(workgroup_root)
        .join(&manifest.name)
        .join(&manifest.version);
    reject_symlinked_directory_components(&dir)
        .map_err(|e| ManifestWriteError::Io(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| ManifestWriteError::Io(e.to_string()))?;
    reject_symlinked_directory_components(&dir)
        .map_err(|e| ManifestWriteError::Io(e.to_string()))?;
    let body = toml::to_string_pretty(manifest)
        .map_err(|e| ManifestWriteError::Serialize(e.to_string()))?;
    let path = dir.join("manifest.toml");
    write_atomic_public(&path, body.as_bytes())
        .map_err(|e| ManifestWriteError::Io(e.to_string()))?;
    Ok(path)
}

/// Refuse to traverse a replicated directory symlink while preparing an image
/// manifest destination. The final file is replaced by rename, so a hostile
/// final-file symlink is also replaced rather than followed; parent symlinks
/// must be rejected before `create_dir_all` can escape the image root.
fn reject_symlinked_directory_components(path: &Path) -> std::io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("refusing symlinked image directory {}", current.display()),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!(
                        "image path component is not a directory: {}",
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

/// Crash-durable public-state replacement. `create_new` makes the temporary
/// inode non-following, and rename replaces (rather than follows) a hostile
/// final symlink. The old manifest therefore remains intact until the new
/// complete body is synced.
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
                format!("invalid image manifest filename {}", path.display()),
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
                format!("temporary image manifest collisions for {}", path.display()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_kinds_round_trip() {
        for k in ImageKind::all() {
            assert_eq!(ImageKind::parse(k.as_str()), Some(k));
            assert!(!k.label().is_empty() && !k.description().is_empty());
        }
        assert_eq!(ImageKind::parse("nope"), None);
    }

    #[test]
    fn load_manifests_walks_versioned_dirs_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let write = |name: &str, ver: &str, body: &str| {
            let dir = images_dir(tmp.path()).join(name).join(ver);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("manifest.toml"), body).unwrap();
        };
        write(
            "cosmic-iso",
            "1.0",
            "name=\"cosmic-iso\"\nkind=\"iso\"\nversion=\"1.0\"\nbuilt_at_ms=1000\n",
        );
        write(
            "cosmic-iso",
            "2.0",
            "name=\"cosmic-iso\"\nkind=\"iso\"\nversion=\"2.0\"\nbuilt_at_ms=2000\n",
        );
        let m = load_manifests(tmp.path());
        assert_eq!(m.len(), 2);
        // Newest (built_at 2000) first.
        assert_eq!(m[0].version, "2.0");
    }

    #[test]
    fn load_manifests_empty_when_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_manifests(tmp.path()).is_empty());
    }

    // ---- W55 record_manifest ------------------------------------

    fn sample(name: &str, kind: &str, ver: &str) -> ImageManifest {
        ImageManifest {
            name: name.into(),
            kind: kind.into(),
            version: ver.into(),
            built_at_ms: Some(1_700_000_000_000),
            size_bytes: Some(4096),
            profile: Some("workstation".into()),
        }
    }

    #[test]
    fn record_manifest_round_trips_through_load() {
        let tmp = tempfile::tempdir().unwrap();
        let m = sample("cosmic-iso", "iso", "3.0");
        let path = record_manifest(&m, tmp.path()).expect("record");
        assert_eq!(
            path,
            images_dir(tmp.path())
                .join("cosmic-iso")
                .join("3.0")
                .join("manifest.toml")
        );
        let loaded = load_manifests(tmp.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], m);
    }

    #[test]
    fn record_manifest_overwrites_same_version_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        record_manifest(&sample("vmgold", "vm", "1.0"), tmp.path()).unwrap();
        let mut m2 = sample("vmgold", "vm", "1.0");
        m2.size_bytes = Some(9999);
        record_manifest(&m2, tmp.path()).unwrap();
        let loaded = load_manifests(tmp.path());
        assert_eq!(loaded.len(), 1, "same version → overwrite, not duplicate");
        assert_eq!(loaded[0].size_bytes, Some(9999));
    }

    #[test]
    fn record_manifest_rejects_bad_kind_name_and_version() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            record_manifest(&sample("x", "floppy", "1.0"), tmp.path()),
            Err(ManifestWriteError::BadKind(_))
        ));
        assert!(matches!(
            record_manifest(&sample("../escape", "iso", "1.0"), tmp.path()),
            Err(ManifestWriteError::BadName(_))
        ));
        assert!(matches!(
            record_manifest(&sample("ok", "iso", ".."), tmp.path()),
            Err(ManifestWriteError::BadVersion(_))
        ));
        assert!(
            load_manifests(tmp.path()).is_empty(),
            "no reject wrote a file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn record_manifest_replaces_final_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let m = sample("cosmic-iso", "iso", "3.0");
        let dir = images_dir(tmp.path()).join(&m.name).join(&m.version);
        std::fs::create_dir_all(&dir).unwrap();
        let victim = tmp.path().join("victim.toml");
        std::fs::write(&victim, b"sentinel").unwrap();
        symlink(&victim, dir.join("manifest.toml")).unwrap();

        record_manifest(&m, tmp.path()).expect("replace safely");

        assert_eq!(std::fs::read(&victim).unwrap(), b"sentinel");
        assert_eq!(load_manifests(tmp.path()), vec![m]);
        assert!(!std::fs::symlink_metadata(dir.join("manifest.toml"))
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
    fn record_manifest_rejects_symlinked_parent_before_escape() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), images_dir(tmp.path())).unwrap();

        let error = record_manifest(&sample("cosmic-iso", "iso", "3.0"), tmp.path())
            .expect_err("symlinked image root must be rejected");
        assert!(matches!(error, ManifestWriteError::Io(_)));
        assert!(!outside.path().join("cosmic-iso/3.0/manifest.toml").exists());
    }

    #[test]
    fn load_manifests_rejects_content_moved_under_another_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = images_dir(tmp.path()).join("catalog-name").join("1.0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            "name = \"different-name\"\nkind = \"iso\"\nversion = \"1.0\"\n",
        )
        .unwrap();

        assert!(load_manifests(tmp.path()).is_empty());
    }
}
