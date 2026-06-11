//! PLANES-22 (W54) — the image-build jobs.
//!
//! Each image kind (ISO / VM golden / container / USB) is produced by a
//! build that runs as a **job on an execution-tagged node** (W54).
//!
//! The build is gated by the `job_exec` execution-tag rail (W84); when its
//! artifact lands it records the manifest the catalog reads.
//!
//! Mirrors the mirrors-sync pattern (PLANES-24): an injectable
//! [`ImageBuildRunner`] (mocked in tests) shells the real per-kind build
//! tool, and [`build_image`] orchestrates build → [`record_manifest`].
//! The orchestration is fully unit-tested; the real [`SubprocessBuild`]
//! commands run on a build node with the tooling installed (livemedia-
//! creator/lorax, podman, virt-builder, qemu-img — the same "real tool,
//! mock-tested orchestration" contract the mirror sync uses).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::image_catalog::{images_dir, record_manifest, ImageKind, ImageManifest};

/// Where an execution node finds the authored build inputs. The RPM
/// installs them under `/usr/share/magic-mesh/`; tests / dev override.
#[derive(Debug, Clone)]
pub struct BuildInputs {
    /// The ISO/USB kickstart (`packaging/kickstart/magic-on-cosmic.ks`).
    pub kickstart: PathBuf,
    /// The mesh-service Containerfile.
    pub containerfile: PathBuf,
    /// The container build context (holds the repo file the Containerfile
    /// COPYs).
    pub context: PathBuf,
}

impl Default for BuildInputs {
    fn default() -> Self {
        let base = PathBuf::from("/usr/share/magic-mesh");
        Self {
            kickstart: base.join("kickstart/magic-on-cosmic.ks"),
            containerfile: base.join("containers/mackesd.Containerfile"),
            context: base,
        }
    }
}

/// The outcome of a build: where the artifact landed + its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltArtifact {
    /// Path to the produced artifact.
    pub path: PathBuf,
    /// Artifact size in bytes.
    pub size_bytes: u64,
}

/// Injectable build runner — shells the real per-kind image tool.
pub trait ImageBuildRunner {
    /// Build `kind` named `name`@`version` into `out_dir`, returning the
    /// produced artifact.
    ///
    /// # Errors
    /// A human-readable string on subprocess / IO failure.
    fn build(
        &self,
        kind: ImageKind,
        name: &str,
        version: &str,
        out_dir: &Path,
    ) -> Result<BuiltArtifact, String>;
}

/// Production runner: shells the real Fedora image tools per kind. Runs on
/// an execution-tagged build node; [`build_image`]'s orchestration is what
/// the unit tests cover (via a mock runner).
pub struct SubprocessBuild {
    /// The authored build inputs (kickstart / Containerfile / context).
    pub inputs: BuildInputs,
}

impl SubprocessBuild {
    /// A runner over the given build inputs.
    #[must_use]
    pub const fn new(inputs: BuildInputs) -> Self {
        Self { inputs }
    }
}

/// Run a build command, mapping a non-zero exit / spawn failure to a
/// human-readable error (the artifact's existence is checked by the
/// caller, so a tool that lies about success still fails the size step).
fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("spawn {cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd} exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

impl ImageBuildRunner for SubprocessBuild {
    fn build(
        &self,
        kind: ImageKind,
        name: &str,
        version: &str,
        out_dir: &Path,
    ) -> Result<BuiltArtifact, String> {
        std::fs::create_dir_all(out_dir)
            .map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;
        let ks = self.inputs.kickstart.to_string_lossy().into_owned();
        let cf = self.inputs.containerfile.to_string_lossy().into_owned();
        let ctx = self.inputs.context.to_string_lossy().into_owned();
        let artifact: PathBuf = match kind {
            ImageKind::Container => {
                // Build the mesh-service image from the authored
                // Containerfile, then save it as a portable OCI archive.
                let tag = format!("{name}:{version}");
                run("podman", &["build", "-t", &tag, "-f", &cf, &ctx])?;
                let art = out_dir.join(format!("{name}-{version}.oci.tar"));
                run("podman", &["save", "-o", &art.to_string_lossy(), &tag])?;
                art
            }
            ImageKind::Iso => {
                // livemedia-creator demands a fresh resultdir, so build
                // into a scratch dir then move the ISO into out_dir.
                let art = out_dir.join(format!("{name}-{version}.iso"));
                let scratch = out_dir.join(".lmc-scratch");
                let _ = std::fs::remove_dir_all(&scratch);
                run(
                    "livemedia-creator",
                    &[
                        "--make-iso",
                        "--ks",
                        &ks,
                        "--resultdir",
                        &scratch.to_string_lossy(),
                        "--project",
                        "Magic Mesh",
                    ],
                )?;
                // livemedia-creator writes the ISO at <resultdir>/images/boot.iso.
                let made = scratch.join("images").join("boot.iso");
                std::fs::rename(&made, &art)
                    .map_err(|e| format!("move {} -> {}: {e}", made.display(), art.display()))?;
                let _ = std::fs::remove_dir_all(&scratch);
                art
            }
            ImageKind::Usb => {
                // The install ISO is isohybrid (dd-able straight to a USB
                // stick), so the USB-writer image is that ISO under a
                // .img name. Reuse the ISO build, then copy.
                let iso = self.build(ImageKind::Iso, name, version, out_dir)?;
                let art = out_dir.join(format!("{name}-{version}.img"));
                std::fs::copy(&iso.path, &art).map_err(|e| format!("copy iso->img: {e}"))?;
                art
            }
            ImageKind::Vm => {
                // Golden qcow2: virt-builder customises a Fedora template,
                // installing the mesh RPM from the project repo.
                let art = out_dir.join(format!("{name}-{version}.qcow2"));
                run(
                    "virt-builder",
                    &[
                        "fedora-42",
                        "--output",
                        &art.to_string_lossy(),
                        "--format",
                        "qcow2",
                        "--copy-in",
                        &format!("{}:/etc/yum.repos.d/", repo_file(&ctx).to_string_lossy()),
                        "--install",
                        "magic-mesh",
                        "--selinux-relabel",
                    ],
                )?;
                art
            }
        };
        let size_bytes = std::fs::metadata(&artifact).map(|m| m.len()).map_err(|e| {
            format!(
                "build reported success but no artifact at {}: {e}",
                artifact.display()
            )
        })?;
        Ok(BuiltArtifact {
            path: artifact,
            size_bytes,
        })
    }
}

/// The shipped dnf repo file inside the build context.
fn repo_file(context: &str) -> PathBuf {
    Path::new(context).join("repo").join("magic-mesh.repo")
}

/// Orchestrate one image build.
///
/// Runs the kind's builder into the catalog's versioned dir, then records
/// the manifest the catalog reads. `now_ms` is injected (not read from the
/// clock) so the orchestration is deterministically testable.
///
/// # Errors
/// A human-readable string at the first failing step (build or record).
pub fn build_image<R: ImageBuildRunner + ?Sized>(
    runner: &R,
    workgroup_root: &Path,
    kind: ImageKind,
    name: &str,
    version: &str,
    profile: Option<String>,
    now_ms: u64,
) -> Result<ImageManifest, String> {
    let out_dir = images_dir(workgroup_root).join(name).join(version);
    let built = runner.build(kind, name, version, &out_dir)?;
    let manifest = ImageManifest {
        name: name.to_string(),
        kind: kind.as_str().to_string(),
        version: version.to_string(),
        built_at_ms: Some(now_ms),
        size_bytes: Some(built.size_bytes),
        profile,
    };
    record_manifest(&manifest, workgroup_root).map_err(|e| format!("record manifest: {e}"))?;
    Ok(manifest)
}

/// Wall-clock now in Unix ms (the CLI entrypoint stamps the manifest).
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock runner that writes a fake artifact (so the size step + the
    /// manifest record exercise the real filesystem) and records the call.
    struct MockRunner {
        fail: bool,
    }
    impl ImageBuildRunner for MockRunner {
        fn build(
            &self,
            kind: ImageKind,
            name: &str,
            version: &str,
            out_dir: &Path,
        ) -> Result<BuiltArtifact, String> {
            if self.fail {
                return Err("simulated build tool failure".into());
            }
            std::fs::create_dir_all(out_dir).unwrap();
            let art = out_dir.join(format!("{name}-{version}.{}", kind.as_str()));
            std::fs::write(&art, b"fake-artifact-bytes").unwrap();
            Ok(BuiltArtifact {
                path: art,
                size_bytes: 19,
            })
        }
    }

    #[test]
    fn build_image_records_the_manifest_after_a_successful_build() {
        let tmp = tempfile::tempdir().unwrap();
        let m = build_image(
            &MockRunner { fail: false },
            tmp.path(),
            ImageKind::Container,
            "mackesd",
            "1.0",
            Some("server".into()),
            1234,
        )
        .unwrap();
        assert_eq!(m.kind, "container");
        assert_eq!(m.size_bytes, Some(19));
        assert_eq!(m.built_at_ms, Some(1234));
        assert_eq!(m.profile.as_deref(), Some("server"));
        // The catalog now sees it.
        let cat = crate::image_catalog::load_manifests(tmp.path());
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0].name, "mackesd");
        assert_eq!(cat[0].version, "1.0");
    }

    #[test]
    fn build_image_propagates_a_build_failure_and_records_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let r = build_image(
            &MockRunner { fail: true },
            tmp.path(),
            ImageKind::Iso,
            "cosmic",
            "2.0",
            None,
            0,
        );
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("simulated build tool failure"));
        // No manifest recorded on failure.
        assert!(crate::image_catalog::load_manifests(tmp.path()).is_empty());
    }

    #[test]
    fn repo_file_sits_under_the_context() {
        assert_eq!(
            repo_file("/usr/share/magic-mesh"),
            Path::new("/usr/share/magic-mesh/repo/magic-mesh.repo")
        );
    }
}
