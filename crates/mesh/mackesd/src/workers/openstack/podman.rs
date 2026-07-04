//! QC-2 — the injectable [`PodmanRunner`] seam: how the `openstack` worker
//! touches Podman, and nothing else does.
//!
//! Production ([`PodmanCli`]) shells `podman` through the bounded
//! [`crate::workers::proc`] path (a wedged podman can't pin a runtime
//! thread), reusing the MV-4 `container` worker's public argv builders +
//! `podman ps` parser (§6 — one podman vocabulary in the tree). Tests drive
//! the in-memory fake in [`super::testkit`].
//!
//! Honesty gates (§7):
//! - a missing `podman` binary answers a typed
//!   [`RunnerError::PodmanAbsent`] (the host image ships podman — Q11/Q35 —
//!   but a dev box may not);
//! - the worker only *checks* image presence ([`PodmanRunner::image_present`])
//!   and never pulls: Kolla images arrive as operator-mirrored archives over
//!   the QC-3 Syncthing lane and are `podman load`ed here
//!   ([`PodmanRunner::load_image`]) **only after** [`super::images`] verifies
//!   the archive against `SHA256SUMS` (design Q18 — no registry on the
//!   airgapped fleet); an absent archive gates the start with a named reason
//!   instead of a doomed pull;
//! - a start additionally requires the rendered Kolla config
//!   ([`config_rendered`]) that QC-4's one-state renderer materializes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use thiserror::Error;

use crate::workers::container::{build_ps_argv, parse_podman_ps, Container};
use crate::workers::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};

use super::catalog::ServiceKind;

/// The Kolla config root on the host — QC-4's renderer materializes
/// `/etc/kolla/<service>/config.json` (+ the service config files) under it.
pub const DEFAULT_KOLLA_CONFIG_ROOT: &str = "/etc/kolla";

/// The bound on one `podman load` (QC-3).
///
/// Kolla archives run to a GiB+ and the load unpacks layers to disk, so
/// the default 15 s command bound would kill every real load; ten minutes
/// is generous for the slowest node while still freeing the converge
/// thread if podman truly wedges. The load runs on the worker's
/// `spawn_blocking` cycle, so a long load never pins the async runtime —
/// it just stretches that one tick.
pub const IMAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// A podman-access failure.
#[derive(Debug, Error)]
pub enum RunnerError {
    /// The `podman` binary is not on this host at all — a typed, named
    /// degrade (never a silent fake success).
    #[error("podman absent: {reason}")]
    PodmanAbsent {
        /// Why/where it was expected.
        reason: String,
    },
    /// The `podman` process couldn't be spawned (other than absence) or
    /// timed out.
    #[error("spawn {0}: {1}")]
    Spawn(String, #[source] std::io::Error),
    /// A command exited non-zero — carries the sub-command, exit code, and
    /// any captured stderr.
    #[error("podman {cmd} failed (exit {code}): {stderr}")]
    Command {
        /// The failing sub-command (first argv element).
        cmd: String,
        /// Process exit code (or -1 if killed by signal).
        code: i32,
        /// Captured stderr (empty for status-only calls).
        stderr: String,
    },
}

/// Map a spawn `io::Error` to the typed runner error: `NotFound` is the
/// honest [`RunnerError::PodmanAbsent`] (the seam's named degrade), anything
/// else a plain spawn failure.
#[must_use]
pub fn spawn_error(err: std::io::Error) -> RunnerError {
    if err.kind() == std::io::ErrorKind::NotFound {
        RunnerError::PodmanAbsent {
            reason: "the `podman` binary is not on PATH — the MCNF host image ships it \
                     (design Q11/Q35); a dev/CI box without it degrades honestly"
                .to_string(),
        }
    } else {
        RunnerError::Spawn("podman".to_string(), err)
    }
}

/// Everything a Kolla service start needs — the spec [`PodmanRunner::run_service`]
/// turns into a `podman run -d` via [`build_kolla_run_argv`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KollaServiceSpec {
    /// Which service.
    pub kind: ServiceKind,
    /// The full local image reference ([`ServiceKind::image_ref`]) — checked
    /// present *before* the start, never pulled.
    pub image: String,
    /// The rendered per-service Kolla config directory
    /// (`<config_root>/<container_name>`).
    pub config_dir: PathBuf,
}

impl KollaServiceSpec {
    /// Build the spec for `kind` at the pinned `release` under `config_root`.
    #[must_use]
    pub fn for_service(kind: ServiceKind, release: &str, config_root: &Path) -> Self {
        Self {
            kind,
            image: kind.image_ref(release),
            config_dir: kolla_config_dir(config_root, kind),
        }
    }
}

/// The per-service Kolla config directory: `<root>/<container_name>`.
#[must_use]
pub fn kolla_config_dir(config_root: &Path, kind: ServiceKind) -> PathBuf {
    config_root.join(kind.container_name())
}

/// Whether QC-4's renderer has materialized this service's Kolla config.
///
/// The marker is the Kolla-convention entrypoint file
/// `<root>/<service>/config.json`. Absent ⇒ the start is honestly gated (the
/// container would crash-loop on an empty config dir, so we never launch it).
#[must_use]
pub fn config_rendered(config_root: &Path, kind: ServiceKind) -> bool {
    kolla_config_dir(config_root, kind)
        .join("config.json")
        .is_file()
}

// ─────────────────────── pure: podman argv builders ───────────────────────
// Each returns the argv WITHOUT the leading `podman`, pure + pinned by tests
// so the command surface can't silently drift (the `mackes-xcp` doctrine).

/// The Kolla container-start argv.
///
/// `podman run -d --name <svc> --net host -e KOLLA_CONFIG_STRATEGY=COPY_ALWAYS
/// -v <config_dir>:/var/lib/kolla/config_files:ro <image>` — the Kolla
/// convention: host networking (the API binds plaintext to the Nebula
/// interface only — Q22/23; the overlay IS the transport security), and the
/// rendered config mounted at Kolla's canonical `config_files` path with the
/// copy-always strategy.
#[must_use]
pub fn build_kolla_run_argv(spec: &KollaServiceSpec) -> Vec<String> {
    vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        spec.kind.container_name().into(),
        "--net".into(),
        "host".into(),
        "-e".into(),
        "KOLLA_CONFIG_STRATEGY=COPY_ALWAYS".into(),
        "-v".into(),
        format!(
            "{}:/var/lib/kolla/config_files:ro",
            spec.config_dir.display()
        ),
        spec.image.clone(),
    ]
}

/// `podman start <name>` — restart an existing (exited/created) container.
#[must_use]
pub fn build_start_argv(name: &str) -> Vec<String> {
    vec!["start".into(), name.into()]
}

/// `podman stop <name>` — graceful stop (SIGTERM→SIGKILL).
#[must_use]
pub fn build_stop_argv(name: &str) -> Vec<String> {
    vec!["stop".into(), name.into()]
}

/// `podman rm <name>` — remove a stopped container.
#[must_use]
pub fn build_rm_argv(name: &str) -> Vec<String> {
    vec!["rm".into(), name.into()]
}

/// `podman image exists <image>` — local-presence check (exit 0 present,
/// exit 1 absent). Never pulls (Q18).
#[must_use]
pub fn build_image_exists_argv(image: &str) -> Vec<String> {
    vec!["image".into(), "exists".into(), image.into()]
}

/// `podman image load -i <archive>` — load a checksum-verified,
/// operator-mirrored Kolla archive from the mesh share (QC-3).
///
/// The docker-archive format `podman save` produced embeds the source tag,
/// so the load restores exactly the [`ServiceKind::image_ref`] the worker
/// gates on. With [`build_image_exists_argv`] these are the only two image
/// verbs in the tree: no pull, ever (Q18).
#[must_use]
pub fn build_image_load_argv(archive: &Path) -> Vec<String> {
    vec![
        "image".into(),
        "load".into(),
        "-i".into(),
        archive.display().to_string(),
    ]
}

// ─────────────────────────── the runner seam ───────────────────────────

/// The injectable podman seam (QC-2). [`PodmanCli`] is the production impl;
/// tests wire [`super::testkit::FakeRunner`].
///
/// Per-container *inspect* is the [`Self::list`] row — `podman ps --all`
/// already carries name/image/state (the same collapse the MV-4 `container`
/// worker documents), so the seam doesn't grow a second state read.
pub trait PodmanRunner {
    /// The node's whole container roster (`podman ps --all`), managed and
    /// unmanaged alike — reconcile filters through the catalog.
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] on a podman-less host; spawn / non-zero
    /// failures otherwise.
    fn list(&self) -> Result<Vec<Container>, RunnerError>;

    /// Whether `image` is present in the local store. Never pulls.
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] / spawn / non-zero failures (absence of
    /// the image is `Ok(false)`, not an error).
    fn image_present(&self, image: &str) -> Result<bool, RunnerError>;

    /// Load an operator-mirrored archive into the local image store
    /// (`podman image load -i`, [`build_image_load_argv`]) — the QC-3
    /// airgap lane's write half.
    ///
    /// Callers MUST only pass an archive
    /// [`super::images::check_archive`] answered
    /// [`super::images::ArchiveStatus::Verified`] for: the checksum gate is
    /// the trust boundary, and this seam never re-checks.
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] / spawn / non-zero failures.
    fn load_image(&self, archive: &Path) -> Result<(), RunnerError>;

    /// Create + start a Kolla service container from `spec`
    /// (`podman run -d`, [`build_kolla_run_argv`]).
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] / spawn / non-zero failures.
    fn run_service(&self, spec: &KollaServiceSpec) -> Result<(), RunnerError>;

    /// Start an existing (exited/created) container — the killed-container
    /// restart leg of the converge.
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] / spawn / non-zero failures.
    fn start_existing(&self, name: &str) -> Result<(), RunnerError>;

    /// Gracefully stop a running container.
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] / spawn / non-zero failures.
    fn stop(&self, name: &str) -> Result<(), RunnerError>;

    /// Remove a stopped container.
    ///
    /// # Errors
    /// [`RunnerError::PodmanAbsent`] / spawn / non-zero failures.
    fn remove(&self, name: &str) -> Result<(), RunnerError>;
}

/// Production [`PodmanRunner`]: shells `podman` through the bounded
/// [`crate::workers::proc`] path. Stateless — every call is a fresh bounded
/// process.
#[derive(Debug, Clone, Default)]
pub struct PodmanCli;

impl PodmanCli {
    /// Construct the production podman runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Run `podman <args>` to completion (status only), bounded by the
    /// default command timeout.
    fn podman_status(args: &[String]) -> Result<(), RunnerError> {
        Self::podman_status_bounded(args, DEFAULT_CMD_TIMEOUT)
    }

    /// Run `podman <args>` to completion (status only) under an explicit
    /// bound — the `load` path needs [`IMAGE_LOAD_TIMEOUT`].
    fn podman_status_bounded(args: &[String], timeout: Duration) -> Result<(), RunnerError> {
        let mut cmd = Command::new("podman");
        cmd.args(args);
        let st = status_with_timeout(cmd, timeout).map_err(spawn_error)?;
        if st.success() {
            Ok(())
        } else {
            Err(RunnerError::Command {
                cmd: args
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "podman".to_string()),
                code: st.code().unwrap_or(-1),
                stderr: String::new(),
            })
        }
    }

    /// Run `podman <args>` capturing stdout/stderr + exit code, bounded.
    fn podman_output(args: &[String]) -> Result<(i32, String, String), RunnerError> {
        let mut cmd = Command::new("podman");
        cmd.args(args);
        let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT).map_err(spawn_error)?;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}

impl PodmanRunner for PodmanCli {
    fn list(&self) -> Result<Vec<Container>, RunnerError> {
        let (code, stdout, stderr) = Self::podman_output(&build_ps_argv())?;
        if code != 0 {
            return Err(RunnerError::Command {
                cmd: "ps".into(),
                code,
                stderr: stderr.trim().to_string(),
            });
        }
        Ok(parse_podman_ps(&stdout))
    }

    fn image_present(&self, image: &str) -> Result<bool, RunnerError> {
        // `podman image exists` speaks in exit codes: 0 present, 1 absent.
        let (code, _stdout, stderr) = Self::podman_output(&build_image_exists_argv(image))?;
        match code {
            0 => Ok(true),
            1 => Ok(false),
            other => Err(RunnerError::Command {
                cmd: "image exists".into(),
                code: other,
                stderr: stderr.trim().to_string(),
            }),
        }
    }

    fn load_image(&self, archive: &Path) -> Result<(), RunnerError> {
        Self::podman_status_bounded(&build_image_load_argv(archive), IMAGE_LOAD_TIMEOUT)
    }

    fn run_service(&self, spec: &KollaServiceSpec) -> Result<(), RunnerError> {
        Self::podman_status(&build_kolla_run_argv(spec))
    }

    fn start_existing(&self, name: &str) -> Result<(), RunnerError> {
        Self::podman_status(&build_start_argv(name))
    }

    fn stop(&self, name: &str) -> Result<(), RunnerError> {
        Self::podman_status(&build_stop_argv(name))
    }

    fn remove(&self, name: &str) -> Result<(), RunnerError> {
        Self::podman_status(&build_rm_argv(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kolla_run_argv_shape() {
        let spec =
            KollaServiceSpec::for_service(ServiceKind::Keystone, "2024.1", Path::new("/etc/kolla"));
        assert_eq!(
            build_kolla_run_argv(&spec),
            vec![
                "run",
                "-d",
                "--name",
                "keystone",
                "--net",
                "host",
                "-e",
                "KOLLA_CONFIG_STRATEGY=COPY_ALWAYS",
                "-v",
                "/etc/kolla/keystone:/var/lib/kolla/config_files:ro",
                "quay.io/openstack.kolla/keystone:2024.1",
            ]
        );
    }

    #[test]
    fn lifecycle_argv_shapes() {
        assert_eq!(build_start_argv("nova_api"), vec!["start", "nova_api"]);
        assert_eq!(build_stop_argv("nova_api"), vec!["stop", "nova_api"]);
        assert_eq!(build_rm_argv("nova_api"), vec!["rm", "nova_api"]);
        assert_eq!(
            build_image_exists_argv("quay.io/openstack.kolla/memcached:2024.1"),
            vec![
                "image",
                "exists",
                "quay.io/openstack.kolla/memcached:2024.1"
            ]
        );
        // QC-3 — the load half of the airgap lane, from the decided share
        // layout.
        assert_eq!(
            build_image_load_argv(Path::new(
                "/mnt/mesh-storage/kolla/2024.1/nova-api-2024.1.tar"
            )),
            vec![
                "image",
                "load",
                "-i",
                "/mnt/mesh-storage/kolla/2024.1/nova-api-2024.1.tar"
            ]
        );
    }

    #[test]
    fn podman_absence_is_typed() {
        // §7 — a podman-less host answers the named PodmanAbsent, never a
        // generic spawn error (and never a fake success).
        let absent = spawn_error(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(
            matches!(&absent, RunnerError::PodmanAbsent { reason } if reason.contains("PATH")),
            "{absent:?}"
        );
        let other = spawn_error(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(matches!(other, RunnerError::Spawn(_, _)), "{other:?}");
    }

    #[test]
    fn config_rendered_requires_the_kolla_entrypoint_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(!config_rendered(root, ServiceKind::Keystone));
        // The bare directory isn't enough — the config.json entrypoint is
        // the render marker.
        std::fs::create_dir_all(root.join("keystone")).unwrap();
        assert!(!config_rendered(root, ServiceKind::Keystone));
        std::fs::write(root.join("keystone").join("config.json"), "{}").unwrap();
        assert!(config_rendered(root, ServiceKind::Keystone));
        // Per-service isolation.
        assert!(!config_rendered(root, ServiceKind::NovaApi));
    }
}
