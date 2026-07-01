//! `mde-kvm` — the local **cloud-hypervisor** VM broker (E12-7, MCNF 12.0 "Quasar").
//!
//! A Workstation runs local VM desktops on **cloud-hypervisor**, the Rust-native
//! VMM (Round-2 lock 11). This crate is the broker between the egui shell's
//! `vm-lifecycle` worker and a `cloud-hypervisor` process:
//!
//! - [`VmSpec`] + [`Nic`] model a VM, including the **dual-homing** lock (19):
//!   every guest is its own Nebula mesh peer *and* carries a LAN-bridged NIC.
//! - [`SharedFolder`] models a **virtio-fs** shared folder (E12-9, the mesh-share
//!   bridge): a host directory (a Syncthing-replicated mesh dir) exported into the
//!   guest, so a file dropped in it appears inside the VM. [`build_ch_config`] folds
//!   each into a cloud-hypervisor `fs` device; the injectable [`VirtiofsLauncher`]
//!   owns the (integration-gated) live `virtiofsd` spawn behind that device.
//! - [`build_ch_config`] is the **load-bearing pure core** — a [`VmSpec`] → the
//!   exact cloud-hypervisor `VmConfig` JSON. It is heavily unit-tested
//!   (spec→JSON correctness, the dual-homed NIC mapping, the virtio-gpu device, the
//!   virtio-fs shared folders) because it is the one place that mapping lives.
//! - [`Vm`] drives the lifecycle (`create`/`boot`/`shutdown`/`info`/`delete`) over
//!   cloud-hypervisor's **HTTP-on-a-unix-socket** API. The transport
//!   ([`ChTransport`]) is injectable, so the lifecycle wiring is unit-tested with
//!   a recording mock; [`Vm::connect`] binds the real [`UnixSocketTransport`].
//!
//! Per §6 this is **glue, not reimplementation**: cloud-hypervisor owns the VMM;
//! this crate only shapes its config and speaks its API. It is dependency-light
//! (serde + a hand-rolled UDS HTTP transport — no hyper/reqwest).
//!
//! ## Integration-gated: the live boot
//!
//! Everything mechanically checkable is pure + unit-tested here. **Actually
//! booting a guest** needs KVM, a running `cloud-hypervisor --api-socket …`, and a
//! golden image — none present on the build farm — so the end-to-end boot is the
//! `#[ignore]`d `tests/live_boot.rs`, gated on `MDE_KVM_TEST_SOCKET`. The
//! lifecycle calls themselves are fully implemented; only the live VMM is parked.
//!
//! The **virtio-fs shared folders** are gated the same way: the [`SharedFolder`]
//! model + the CH `fs` mapping are pure + unit-tested, but spawning the live
//! `virtiofsd` (which needs the binary + the host mesh-share export) is parked behind
//! [`VirtiofsLauncher`], whose production impl ([`LiveVirtiofsLauncher`]) returns a
//! typed integration-gated error rather than faking success.

mod config;
mod spec;
mod transport;
mod virtiofs;
mod vm;

use std::path::PathBuf;

use thiserror::Error;

pub use config::build_ch_config;
pub use spec::{
    api_socket_path, gpu_socket_path, running_disk_path, virtiofs_socket_path, Nic, NicRole,
    SharedFolder, VmSpec, DEFAULT_FIRMWARE, MESH_SHARE_TAG, RUNTIME_DIR,
};
pub use transport::{
    build_http_request, parse_http_response, ChResponse, ChTransport, UnixSocketTransport,
};
pub use virtiofs::{LiveVirtiofsLauncher, VirtiofsError, VirtiofsLauncher};
pub use vm::{Vm, VmInfo};

/// A VM-broker failure.
#[derive(Debug, Error)]
pub enum KvmError {
    /// Could not connect to the cloud-hypervisor api-socket (the VMM isn't
    /// listening, or the path is wrong).
    #[error("connect cloud-hypervisor api-socket {0}: {1}")]
    Connect(PathBuf, #[source] std::io::Error),
    /// An I/O error talking to the api-socket after connecting.
    #[error("api-socket io: {0}")]
    Io(#[from] std::io::Error),
    /// The api-socket spoke something that wasn't a parseable HTTP response.
    #[error("api protocol: {0}")]
    Protocol(String),
    /// cloud-hypervisor returned a non-2xx status for an API verb — carries the
    /// verb, path, status, and the VMM's error body.
    #[error("cloud-hypervisor api {method} {path} failed (status {status}): {body}")]
    Api {
        /// The HTTP method (`PUT`/`GET`).
        method: String,
        /// The API path (e.g. `/api/v1/vm.create`).
        path: String,
        /// The HTTP status code returned.
        status: u16,
        /// The VMM's error body (its diagnostic text).
        body: String,
    },
    /// (De)serializing a `VmConfig` / `vm.info` body failed.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
