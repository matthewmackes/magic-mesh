//! The virtio-fs shared-folder launcher seam (E12-9, the mesh-share bridge).
//!
//! A [`SharedFolder`] is exported into the guest by a **virtiofsd** process bound to
//! a per-folder unix socket; cloud-hypervisor's `fs` device (emitted by
//! [`build_ch_config`](crate::build_ch_config)) then connects to that socket and the
//! guest mounts it under the folder's tag. Spawning virtiofsd — which needs the live
//! `virtiofsd` binary and a real host mesh-share export — is the **integration-gated**
//! side effect, so it sits behind the injectable [`VirtiofsLauncher`] trait, exactly
//! as the [`ChTransport`](crate::ChTransport) seam isolates the live VMM socket.
//!
//! Production wires [`LiveVirtiofsLauncher`]; until the live virtiofsd + host export
//! are wired it returns a typed [`VirtiofsError::IntegrationGated`] naming exactly what
//! is missing — never a fake success (§7), mirroring the parked live VMM boot. Tests
//! inject an in-memory fake so the wiring is exercised without a real virtiofsd.

use std::path::PathBuf;

use crate::spec::SharedFolder;

/// A typed failure from the [`VirtiofsLauncher`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtiofsError {
    /// The live virtiofsd launch is not runnable in this build/environment yet — it
    /// needs a real prerequisite (the `virtiofsd` binary + the host mesh-share
    /// export). Names the op + what is missing. §7-legal: a real method returning a
    /// real typed error — exactly as the live VMM boot is parked — never a fake
    /// success.
    IntegrationGated {
        /// Which seam op (`launch`).
        op: &'static str,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A seam op failed for a concrete runtime reason (e.g. virtiofsd spawned but
    /// exited, or the export dir was gone).
    Failed {
        /// Which seam op failed.
        op: &'static str,
        /// The failure detail.
        reason: String,
    },
}

impl std::fmt::Display for VirtiofsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { op, reason } => {
                write!(f, "{op}: integration-gated — {reason}")
            }
            Self::Failed { op, reason } => write!(f, "{op}: {reason}"),
        }
    }
}

impl std::error::Error for VirtiofsError {}

/// The injectable virtio-fs launcher: start a virtiofsd exporting a [`SharedFolder`]'s
/// host directory and return the unix socket cloud-hypervisor's `fs` device connects
/// to.
///
/// Production is [`LiveVirtiofsLauncher`]; tests inject an in-memory fake so the
/// wiring is exercised without a live virtiofsd. The returned socket is exactly the
/// one [`build_ch_config`](crate::build_ch_config) put in the guest's `fs` device —
/// both ends derive it from [`virtiofs_socket_path`](crate::virtiofs_socket_path), so
/// they agree by construction.
pub trait VirtiofsLauncher {
    /// Launch a virtiofsd for `folder` on VM `vm_name`: export
    /// [`folder.host_path`](SharedFolder::host_path) (read-only iff
    /// [`folder.read_only`](SharedFolder::read_only)) and bind it to the folder's
    /// per-VM socket ([`virtiofs_socket_path`](crate::virtiofs_socket_path)). Returns
    /// that socket path.
    ///
    /// # Errors
    /// A [`VirtiofsError`] — `IntegrationGated` until the live virtiofsd + host
    /// mesh-share export are wired, else `Failed`.
    fn launch(&self, vm_name: &str, folder: &SharedFolder) -> Result<PathBuf, VirtiofsError>;
}

/// Production [`VirtiofsLauncher`]: spawns the real `virtiofsd`.
///
/// This slice (E12-9) delivers the [`SharedFolder`] model + the CH `fs` mapping + this
/// seam; the live executor — spawning `virtiofsd --socket-path <sock> --shared-dir
/// <host_path> [--readonly]` and reaping it with the VM — is wired by a later E12 unit
/// (it needs the live virtiofsd binary + the host mesh-share export, neither present
/// on the build farm). Until then [`launch`](VirtiofsLauncher::launch) returns a typed
/// [`VirtiofsError::IntegrationGated`] naming exactly what the live call needs — never
/// a fake success (§7), mirroring the parked live VMM boot.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveVirtiofsLauncher;

impl VirtiofsLauncher for LiveVirtiofsLauncher {
    fn launch(&self, _vm_name: &str, folder: &SharedFolder) -> Result<PathBuf, VirtiofsError> {
        Err(VirtiofsError::IntegrationGated {
            op: "launch",
            reason: format!(
                "shared folder '{tag}' (host {host}) → needs the live virtiofsd binary + the \
                 host mesh-share export (spawn `virtiofsd --socket-path … --shared-dir {host}`); \
                 the virtiofsd process + guest mount isn't wired yet",
                tag = folder.tag,
                host = folder.host_path.display(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{virtiofs_socket_path, MESH_SHARE_TAG};
    use std::cell::RefCell;

    /// An in-memory [`VirtiofsLauncher`] — the Fake seam. Records each launch and
    /// returns the deterministic per-folder socket, so the wiring is exercised without
    /// a live virtiofsd (mirrors `vm.rs`'s recording `MockTransport`).
    #[derive(Default)]
    struct FakeVirtiofs {
        launches: RefCell<Vec<(String, SharedFolder)>>,
    }

    impl VirtiofsLauncher for FakeVirtiofs {
        fn launch(&self, vm_name: &str, folder: &SharedFolder) -> Result<PathBuf, VirtiofsError> {
            self.launches
                .borrow_mut()
                .push((vm_name.to_string(), folder.clone()));
            Ok(virtiofs_socket_path(vm_name, &folder.tag))
        }
    }

    #[test]
    fn live_launcher_is_integration_gated_never_fake_success() {
        let live = LiveVirtiofsLauncher;
        let folder = SharedFolder::mesh_share("/home/op/Mesh/Share");
        let err = live
            .launch("web1", &folder)
            .expect_err("the live virtiofsd launch must be integration-gated, not a fake success");
        // a typed IntegrationGated whose reason names the tag, the host export, and the
        // missing virtiofsd binary — never a fake success (§7).
        assert!(
            matches!(
                &err,
                VirtiofsError::IntegrationGated { op, reason }
                    if *op == "launch"
                        && reason.contains(MESH_SHARE_TAG)
                        && reason.contains("/home/op/Mesh/Share")
                        && reason.contains("virtiofsd")
            ),
            "expected an integration-gated error naming the tag/host/binary, got {err:?}"
        );
        // the rendered error carries the op + the integration-gated marker.
        let msg = err.to_string();
        assert!(msg.contains("launch"), "{msg}");
        assert!(msg.contains("integration-gated"), "{msg}");
    }

    #[test]
    fn fake_launcher_returns_the_per_folder_socket_and_records_the_launch() {
        let fake = FakeVirtiofs::default();
        let folder = SharedFolder::mesh_share("/srv/mesh");
        let sock = fake.launch("db1", &folder).expect("fake launch");
        // the returned socket is exactly the one build_ch_config's `fs` device dials.
        assert_eq!(sock, virtiofs_socket_path("db1", MESH_SHARE_TAG));
        let recorded = fake.launches.borrow();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "db1");
        assert_eq!(recorded[0].1, folder);
    }

    #[test]
    fn error_display_distinguishes_gated_from_failed() {
        let gated = VirtiofsError::IntegrationGated {
            op: "launch",
            reason: "needs virtiofsd".to_string(),
        };
        assert_eq!(
            gated.to_string(),
            "launch: integration-gated — needs virtiofsd"
        );
        let failed = VirtiofsError::Failed {
            op: "launch",
            reason: "no such export dir".to_string(),
        };
        assert_eq!(failed.to_string(), "launch: no such export dir");
    }
}
