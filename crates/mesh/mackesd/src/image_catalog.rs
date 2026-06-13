//! PLANES-22 — the image catalog (W53/W54/W55).
//!
//! The mesh builds four kinds of image (W53): the install **ISO** (+
//! kickstart), the **VM** golden image, **container** images, and the
//! **USB** writer image. Each build is a job on an execution-tagged node
//! (W54), and its output lands as a versioned dir with a TOML manifest on
//! LizardFS (`<root>/images/<name>/<version>/manifest.toml`, W55).
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
        if !name_entry.path().is_dir() {
            continue;
        }
        let Ok(versions) = std::fs::read_dir(name_entry.path()) else {
            continue;
        };
        for ver_entry in versions.filter_map(Result::ok) {
            let manifest = ver_entry.path().join("manifest.toml");
            if let Ok(raw) = std::fs::read_to_string(&manifest) {
                if let Ok(m) = toml::from_str::<ImageManifest>(&raw) {
                    out.push(m);
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
// that load_manifests + the Images panel then surface. LizardFS
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
    std::fs::create_dir_all(&dir).map_err(|e| ManifestWriteError::Io(e.to_string()))?;
    let body = toml::to_string_pretty(manifest)
        .map_err(|e| ManifestWriteError::Serialize(e.to_string()))?;
    let path = dir.join("manifest.toml");
    std::fs::write(&path, body).map_err(|e| ManifestWriteError::Io(e.to_string()))?;
    Ok(path)
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
}
