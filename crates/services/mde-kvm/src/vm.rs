//! The VM lifecycle: `create` / `boot` / `shutdown` / `info` / `delete` over the
//! cloud-hypervisor API.
//!
//! [`Vm`] is generic over a [`ChTransport`], so the lifecycle wiring — which verb
//! maps to which method+path, that `vm.create` carries the [`build_ch_config`]
//! body, that a non-2xx surfaces as [`KvmError::Api`] — is unit-tested through a
//! recording mock. [`Vm::connect`] binds the real [`UnixSocketTransport`].
//!
//! **Live boot is integration-gated.** Actually launching a guest needs KVM, a
//! running `cloud-hypervisor --api-socket …`, and a golden image — none of which
//! exist on the build farm. The lifecycle calls are fully implemented; the
//! end-to-end boot is exercised by the `#[ignore]`d `tests/live_boot.rs` against a
//! real socket. See that file for the gate.

use serde::Deserialize;

use crate::config::build_ch_config;
use crate::spec::VmSpec;
use crate::transport::{ChResponse, ChTransport, UnixSocketTransport};
use crate::KvmError;

/// cloud-hypervisor's API base path; every verb hangs off it.
const API_BASE: &str = "/api/v1";

/// A handle to one cloud-hypervisor instance, bound to its api-socket transport.
///
/// One `cloud-hypervisor` process (hence one api-socket, hence one `Vm`) hosts a
/// single guest, so the lifecycle verbs take no VM id — the socket *is* the VM.
#[derive(Debug, Clone)]
pub struct Vm<T: ChTransport> {
    transport: T,
}

impl Vm<UnixSocketTransport> {
    /// Bind to a cloud-hypervisor instance via its api-socket at `socket`
    /// (e.g. [`crate::spec::api_socket_path`] for a VM name).
    #[must_use]
    pub fn connect(socket: impl Into<std::path::PathBuf>) -> Self {
        Self {
            transport: UnixSocketTransport::new(socket),
        }
    }
}

impl<T: ChTransport> Vm<T> {
    /// Bind to an arbitrary transport (the test seam).
    pub const fn with_transport(transport: T) -> Self {
        Self { transport }
    }

    /// `PUT /api/v1/vm.create` with `spec`'s `VmConfig` body — defines the guest
    /// (it is not yet running; follow with [`Vm::boot`]).
    ///
    /// A spec carrying VFIO passthrough devices without the explicit operator
    /// opt-in ([`VmSpec::vfio_allowed`], lock 13) is **refused** before any
    /// transport call — passthrough hands the guest raw DMA-capable hardware,
    /// so it never rides an un-opted create. (Host readiness — IOMMU, the
    /// vfio-pci binding, group viability — is [`crate::preflight_vfio`]'s job,
    /// which the vm-lifecycle caller runs first; this pure gate is the
    /// backstop that cannot be skipped.)
    ///
    /// # Errors
    /// [`KvmError::Vfio`] for an un-opted passthrough spec; otherwise
    /// serialization, transport, or a non-2xx API response.
    pub fn create(&self, spec: &VmSpec) -> Result<(), KvmError> {
        crate::vfio::ensure_vfio_opt_in(spec)?;
        let body = serde_json::to_string(&build_ch_config(spec))?;
        self.put("/vm.create", Some(&body)).map(|_| ())
    }

    /// `PUT /api/v1/vm.boot` — start the defined guest.
    ///
    /// # Errors
    /// Transport or a non-2xx API response.
    pub fn boot(&self) -> Result<(), KvmError> {
        self.put("/vm.boot", None).map(|_| ())
    }

    /// `PUT /api/v1/vm.shutdown` — stop the guest (the definition remains; it can
    /// be booted again). Per lock 30 a disconnected VM otherwise *keeps running* —
    /// this is the explicit stop.
    ///
    /// # Errors
    /// Transport or a non-2xx API response.
    pub fn shutdown(&self) -> Result<(), KvmError> {
        self.put("/vm.shutdown", None).map(|_| ())
    }

    /// `PUT /api/v1/vm.delete` — remove the guest definition from the VMM.
    ///
    /// # Errors
    /// Transport or a non-2xx API response.
    pub fn delete(&self) -> Result<(), KvmError> {
        self.put("/vm.delete", None).map(|_| ())
    }

    /// `PUT /api/v1/vm.add-disk` — hot-plug the image at `path` into the running
    /// guest as a virtio-blk device pinned to `id` (E12-22 attach). Reuses the live
    /// guest — no lifecycle reimplementation; the disk id lets [`Vm::remove_device`]
    /// detach exactly this image later.
    ///
    /// # Errors
    /// Serialization, transport, or a non-2xx API response.
    pub fn add_disk(
        &self,
        id: &str,
        path: &std::path::Path,
        readonly: bool,
    ) -> Result<(), KvmError> {
        let body = serde_json::to_string(&crate::config::disk_hotplug_body(id, path, readonly))?;
        self.put("/vm.add-disk", Some(&body)).map(|_| ())
    }

    /// `PUT /api/v1/vm.remove-device` — detach the hot-plugged device named `id`
    /// (E12-22 detach).
    ///
    /// # Errors
    /// Serialization, transport, or a non-2xx API response.
    pub fn remove_device(&self, id: &str) -> Result<(), KvmError> {
        let body = serde_json::to_string(&crate::config::remove_device_body(id))?;
        self.put("/vm.remove-device", Some(&body)).map(|_| ())
    }

    /// `GET /api/v1/vm.info` — the guest's current state + effective config.
    ///
    /// # Errors
    /// Transport, a non-2xx API response, or an unparseable body.
    pub fn info(&self) -> Result<VmInfo, KvmError> {
        let resp = self.request("GET", "/vm.info", None)?;
        VmInfo::from_json(&resp.body)
    }

    /// A checked `PUT` (shared by the bodyless verbs + `vm.create`, and by the
    /// migration executor in [`crate::migrate`], which drives the
    /// `vm.receive-migration`/`vm.send-migration` verbs through this same
    /// non-2xx mapping).
    pub(crate) fn put(&self, endpoint: &str, body: Option<&str>) -> Result<ChResponse, KvmError> {
        self.request("PUT", endpoint, body)
    }

    /// Issue one request and map a non-2xx response to [`KvmError::Api`].
    fn request(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&str>,
    ) -> Result<ChResponse, KvmError> {
        let path = format!("{API_BASE}{endpoint}");
        let resp = self.transport.request(method, &path, body)?;
        if resp.is_success() {
            Ok(resp)
        } else {
            Err(KvmError::Api {
                method: method.to_string(),
                path,
                status: resp.status,
                body: resp.body,
            })
        }
    }
}

/// The slice of cloud-hypervisor's `vm.info` we consume: the run `state` and the
/// effective `config`. Unknown fields (memory sizes, device trees, …) are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct VmInfo {
    /// One of cloud-hypervisor's states: `Created` / `Running` / `Shutdown` /
    /// `Paused` / `BreakPoint`.
    pub state: String,
    /// The effective `VmConfig` cloud-hypervisor is running, as reported back.
    #[serde(default)]
    pub config: serde_json::Value,
}

impl VmInfo {
    /// Parse a `vm.info` JSON body.
    ///
    /// # Errors
    /// Malformed JSON / a missing `state` field.
    pub fn from_json(body: &str) -> Result<Self, KvmError> {
        serde_json::from_str(body).map_err(KvmError::from)
    }

    /// Whether the guest is currently running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state.eq_ignore_ascii_case("running")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::build_ch_config;
    use crate::spec::{Nic, VmSpec};
    use std::cell::RefCell;

    /// One recorded transport call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Call {
        method: String,
        path: String,
        body: Option<String>,
    }

    /// A recording transport: captures every call and replays a canned response.
    struct MockTransport {
        calls: RefCell<Vec<Call>>,
        status: u16,
        body: String,
    }

    impl MockTransport {
        fn ok() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                status: 204,
                body: String::new(),
            }
        }

        fn responding(status: u16, body: &str) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                status,
                body: body.to_string(),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl ChTransport for MockTransport {
        fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<&str>,
        ) -> Result<ChResponse, KvmError> {
            self.calls.borrow_mut().push(Call {
                method: method.to_string(),
                path: path.to_string(),
                body: body.map(str::to_string),
            });
            Ok(ChResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }
    }

    fn spec() -> VmSpec {
        VmSpec::new("web1", 2, 2048, "/home/op/Local/web1.img")
            .with_virtio_gpu(true)
            .with_nic(Nic::mesh("mvm-web1-mesh"))
            .with_nic(Nic::lan("mvm-web1-lan"))
    }

    #[test]
    fn create_puts_vm_create_with_the_built_config_body() {
        let vm = Vm::with_transport(MockTransport::ok());
        vm.create(&spec()).expect("create");
        let calls = vm.transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "PUT");
        assert_eq!(calls[0].path, "/api/v1/vm.create");
        // The body is exactly the serialized build_ch_config(spec).
        let sent: serde_json::Value =
            serde_json::from_str(calls[0].body.as_ref().expect("body")).expect("json");
        assert_eq!(sent, build_ch_config(&spec()));
    }

    #[test]
    fn boot_shutdown_delete_are_bodyless_puts_on_the_right_paths() {
        type Verb = fn(&Vm<MockTransport>) -> Result<(), KvmError>;
        let cases: [(Verb, &str); 3] = [
            (|v| v.boot(), "/api/v1/vm.boot"),
            (|v| v.shutdown(), "/api/v1/vm.shutdown"),
            (|v| v.delete(), "/api/v1/vm.delete"),
        ];
        for (verb, path) in cases {
            let vm = Vm::with_transport(MockTransport::ok());
            verb(&vm).expect("verb");
            let calls = vm.transport.calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].method, "PUT");
            assert_eq!(calls[0].path, path);
            assert_eq!(calls[0].body, None, "{path} must be bodyless");
        }
    }

    #[test]
    fn info_gets_vm_info_and_parses_state() {
        let vm = Vm::with_transport(MockTransport::responding(
            200,
            r#"{"state":"Running","config":{"cpus":{"boot_vcpus":2}},"memory_actual_size":2147483648}"#,
        ));
        let info = vm.info().expect("info");
        assert!(info.is_running());
        assert_eq!(info.state, "Running");
        // Unknown fields (memory_actual_size) are ignored; config rides through.
        assert_eq!(info.config["cpus"]["boot_vcpus"], serde_json::json!(2));
        let calls = vm.transport.calls();
        assert_eq!(calls[0].method, "GET");
        assert_eq!(calls[0].path, "/api/v1/vm.info");
    }

    #[test]
    fn create_refuses_un_opted_vfio_passthrough_before_any_transport_call() {
        use crate::vfio::{PciAddress, VfioDevice, VfioError};
        let gpu = VfioDevice::new(PciAddress::parse("0000:01:00.0").expect("addr"));
        let vm = Vm::with_transport(MockTransport::ok());
        // no opt-in → typed refusal, and the VMM is never touched.
        let err = vm
            .create(&spec().with_vfio_device(gpu.clone()))
            .expect_err("must refuse un-opted passthrough");
        assert!(
            matches!(&err, KvmError::Vfio(VfioError::NotOptedIn { .. })),
            "{err:?}"
        );
        assert!(vm.transport.calls().is_empty(), "no transport call allowed");
        // with the operator opt-in the create proceeds and carries the device.
        let vm = Vm::with_transport(MockTransport::ok());
        vm.create(&spec().with_vfio_device(gpu).allow_vfio(true))
            .expect("opted-in create");
        let calls = vm.transport.calls();
        assert_eq!(calls.len(), 1);
        let sent: serde_json::Value =
            serde_json::from_str(calls[0].body.as_ref().expect("body")).expect("json");
        assert_eq!(
            sent["devices"][0]["path"],
            serde_json::json!("/sys/bus/pci/devices/0000:01:00.0")
        );
    }

    #[test]
    fn non_2xx_surfaces_as_an_api_error_with_status_and_body() {
        let vm = Vm::with_transport(MockTransport::responding(500, "vmm: bad config"));
        let err = vm.create(&spec()).expect_err("should fail");
        assert!(
            matches!(&err, KvmError::Api { status: 500, .. }),
            "expected a 500 Api error, got {err:?}"
        );
        // The rendered error carries the method, path, status, and VMM body.
        let msg = err.to_string();
        assert!(msg.contains("PUT"), "{msg}");
        assert!(msg.contains("/api/v1/vm.create"), "{msg}");
        assert!(msg.contains("500"), "{msg}");
        assert!(msg.contains("vmm: bad config"), "{msg}");
    }

    #[test]
    fn add_disk_and_remove_device_put_the_hotplug_bodies() {
        // attach: PUT vm.add-disk carrying the DiskConfig (path + readonly + id).
        let vm = Vm::with_transport(MockTransport::ok());
        vm.add_disk(
            "disk-data",
            std::path::Path::new("/home/op/Local/data.qcow2"),
            false,
        )
        .expect("add-disk");
        let calls = vm.transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "PUT");
        assert_eq!(calls[0].path, "/api/v1/vm.add-disk");
        let sent: serde_json::Value =
            serde_json::from_str(calls[0].body.as_ref().expect("body")).expect("json");
        assert_eq!(sent["path"], serde_json::json!("/home/op/Local/data.qcow2"));
        assert_eq!(sent["readonly"], serde_json::json!(false));
        assert_eq!(sent["id"], serde_json::json!("disk-data"));

        // detach: PUT vm.remove-device carrying just the id.
        let vm = Vm::with_transport(MockTransport::ok());
        vm.remove_device("disk-data").expect("remove-device");
        let calls = vm.transport.calls();
        assert_eq!(calls[0].path, "/api/v1/vm.remove-device");
        let sent: serde_json::Value =
            serde_json::from_str(calls[0].body.as_ref().expect("body")).expect("json");
        assert_eq!(sent, serde_json::json!({"id": "disk-data"}));
    }

    #[test]
    fn vm_info_not_running_when_shutdown() {
        let info = VmInfo::from_json(r#"{"state":"Shutdown"}"#).expect("parse");
        assert!(!info.is_running());
        assert_eq!(info.state, "Shutdown");
    }
}
