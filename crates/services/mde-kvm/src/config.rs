//! The load-bearing pure core: a [`VmSpec`] → cloud-hypervisor `VmConfig` JSON.
//!
//! cloud-hypervisor's API de-serializes the `vm.create` request body into its
//! internal `VmConfig`. [`build_ch_config`] is the *only* place that mapping
//! lives, and it is a pure function so the exact spec→JSON shape — including the
//! dual-homed NIC mapping and the virtio-gpu device — is unit-tested without a
//! live VMM. Field names track cloud-hypervisor's OpenAPI `VmConfig`:
//! `cpus`/`memory`/`payload`/`disks`/`net`/`rng`/`serial`/`console`, plus the
//! vhost-user `gpu` device.

use serde_json::{json, Value};

use crate::spec::{gpu_socket_path, virtiofs_socket_path, Nic, SharedFolder, VmSpec};

/// MiB → bytes (cloud-hypervisor's `memory.size` is bytes).
const MIB: u64 = 1024 * 1024;

/// cloud-hypervisor virtio-fs queue sizing (its `FsConfig` defaults): a single
/// request virtqueue, 1024 descriptors deep — ample for a desktop mesh-share.
const FS_NUM_QUEUES: usize = 1;
const FS_QUEUE_SIZE: u16 = 1024;

/// Build the cloud-hypervisor `VmConfig` JSON for `spec`.
///
/// Mapping:
/// - `cpus` ← `{ boot_vcpus, max_vcpus }`, both = [`VmSpec::vcpus`] (fixed-sized
///   local desktops, lock 17 — no hotplug headroom).
/// - `memory.size` ← [`VmSpec::mem_mib`] × 1 MiB, in bytes.
/// - `payload.firmware` ← [`VmSpec::firmware_path`] (UEFI boot of the golden disk).
/// - `disks[0]` ← the running disk, opened read-write (lock 18).
/// - `net` ← one entry per [`VmSpec::nics`] (order preserved; the dual-homing).
/// - `gpu` ← a vhost-user-gpu device at [`gpu_socket_path`] when
///   [`VmSpec::virtio_gpu`] (lock 12); the `console` is then `Off` because the
///   GPU is the display.
/// - `fs` ← one entry per [`VmSpec::shared_folders`] (E12-9, the mesh-share bridge):
///   the guest mount `tag` + the per-folder virtiofsd `socket`
///   ([`virtiofs_socket_path`]). The host `host_path`/`read_only` are virtiofsd's
///   concern (the live launch), not cloud-hypervisor's, so they are not emitted.
/// - `rng`/`serial` ← entropy + a debug serial, always present.
#[must_use]
pub fn build_ch_config(spec: &VmSpec) -> Value {
    let mut config = json!({
        "cpus": {
            "boot_vcpus": spec.vcpus,
            "max_vcpus": spec.vcpus,
        },
        "memory": {
            "size": spec.mem_mib * MIB,
        },
        "payload": {
            "firmware": spec.firmware_path().to_string_lossy(),
        },
        "disks": [
            { "path": spec.disk.to_string_lossy(), "readonly": false },
        ],
        "rng": { "src": "/dev/urandom" },
        "serial": { "mode": "Tty" },
        // With virtio-gpu the display IS the GPU, so the virtio-console is off;
        // otherwise expose a Tty console for headless/text guests.
        "console": { "mode": if spec.virtio_gpu { "Off" } else { "Tty" } },
    });

    // The dual-homed NICs (lock 19): each guest NIC → one `net` entry. Both the
    // mesh-peer and LAN-bridged NICs are host taps to cloud-hypervisor; the role
    // only drives host-side bridge enslavement, so it does not appear here.
    if !spec.nics.is_empty() {
        let net: Vec<Value> = spec.nics.iter().map(nic_to_net).collect();
        config["net"] = Value::Array(net);
    }

    // The virtio-gpu zero-copy fast path (lock 12): a vhost-user device pointed at
    // the per-VM gpu socket, where the shell's GPU backend exports a dmabuf.
    if spec.virtio_gpu {
        config["gpu"] = json!([
            { "socket": gpu_socket_path(&spec.name).to_string_lossy() },
        ]);
    }

    // The virtio-fs shared folders (E12-9, the mesh-share bridge): each folder → a
    // cloud-hypervisor `fs` device pointed at its per-folder virtiofsd socket. The
    // host `host_path`/`read_only` drive the virtiofsd launch, so — like the NIC
    // role — they do not appear here; only the guest mount `tag` + the socket the
    // device dials.
    if !spec.shared_folders.is_empty() {
        let fs: Vec<Value> = spec
            .shared_folders
            .iter()
            .map(|sf| shared_folder_to_fs(&spec.name, sf))
            .collect();
        config["fs"] = Value::Array(fs);
    }

    config
}

/// One [`Nic`] → a cloud-hypervisor `NetConfig` entry. Only `tap` (always) plus
/// `mac`/`mtu` (when set) are emitted; `ip`/`mask` are left to cloud-hypervisor's
/// defaults because a bridged tap takes its address from the host bridge / DHCP,
/// not cloud-hypervisor's in-VMM L2.
fn nic_to_net(nic: &Nic) -> Value {
    let mut entry = json!({ "tap": nic.tap });
    if let Some(mac) = &nic.mac {
        entry["mac"] = json!(mac);
    }
    if let Some(mtu) = nic.mtu {
        entry["mtu"] = json!(mtu);
    }
    entry
}

/// One [`SharedFolder`] → a cloud-hypervisor `FsConfig` entry: the guest mount `tag`,
/// the per-folder virtiofsd `socket` ([`virtiofs_socket_path`], derived from the VM
/// name + tag so it matches the launcher's binding), and the queue sizing. The host
/// `host_path` + `read_only` are virtiofsd's concern (the [`VirtiofsLauncher`] launch),
/// not cloud-hypervisor's, so they are deliberately not emitted here.
///
/// [`VirtiofsLauncher`]: crate::VirtiofsLauncher
fn shared_folder_to_fs(vm_name: &str, folder: &SharedFolder) -> Value {
    json!({
        "tag": folder.tag,
        "socket": virtiofs_socket_path(vm_name, &folder.tag).to_string_lossy(),
        "num_queues": FS_NUM_QUEUES,
        "queue_size": FS_QUEUE_SIZE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{
        gpu_socket_path, virtiofs_socket_path, Nic, SharedFolder, VmSpec, DEFAULT_FIRMWARE,
        MESH_SHARE_TAG,
    };

    /// A representative dual-homed desktop spec for the mapping tests.
    fn web1() -> VmSpec {
        VmSpec::new("web1", 4, 4096, "/home/op/Local/web1.img")
            .with_virtio_gpu(true)
            .with_nic(Nic::mesh("mvm-web1-mesh").with_mac("02:00:00:00:00:01"))
            .with_nic(Nic::lan("mvm-web1-lan").with_mac("02:00:00:00:00:02"))
    }

    #[test]
    fn cpus_memory_payload_disk_are_mapped() {
        let cfg = build_ch_config(&web1());
        // cpus: boot == max == vcpus (fixed-sized).
        assert_eq!(cfg["cpus"]["boot_vcpus"], json!(4));
        assert_eq!(cfg["cpus"]["max_vcpus"], json!(4));
        // memory.size is MiB → bytes.
        assert_eq!(cfg["memory"]["size"], json!(4096u64 * 1024 * 1024));
        // payload firmware defaults to CLOUDHV.fd.
        assert_eq!(cfg["payload"]["firmware"], json!(DEFAULT_FIRMWARE));
        // the single running disk, read-write.
        assert_eq!(cfg["disks"][0]["path"], json!("/home/op/Local/web1.img"));
        assert_eq!(cfg["disks"][0]["readonly"], json!(false));
        // always-present entropy + debug serial.
        assert_eq!(cfg["rng"]["src"], json!("/dev/urandom"));
        assert_eq!(cfg["serial"]["mode"], json!("Tty"));
    }

    #[test]
    fn dual_homed_nics_map_to_net_in_order() {
        // THE dual-homing acceptance: mesh NIC first (eth0), LAN NIC second, each
        // a `net` entry carrying its own tap + MAC.
        let cfg = build_ch_config(&web1());
        let net = cfg["net"].as_array().expect("net array");
        assert_eq!(net.len(), 2);
        assert_eq!(net[0]["tap"], json!("mvm-web1-mesh"));
        assert_eq!(net[0]["mac"], json!("02:00:00:00:00:01"));
        assert_eq!(net[1]["tap"], json!("mvm-web1-lan"));
        assert_eq!(net[1]["mac"], json!("02:00:00:00:00:02"));
        // ip/mask are NOT pinned (bridged taps take their address from the host).
        assert!(net[0].get("ip").is_none());
        assert!(net[0].get("mask").is_none());
    }

    #[test]
    fn mtu_is_emitted_only_when_set() {
        let spec = VmSpec::new("m", 1, 512, "/d.img")
            .with_nic(Nic::mesh("t-mesh").with_mtu(1300))
            .with_nic(Nic::lan("t-lan"));
        let cfg = build_ch_config(&spec);
        let net = cfg["net"].as_array().expect("net");
        // mesh NIC pins the Nebula overlay MTU…
        assert_eq!(net[0]["mtu"], json!(1300));
        // …the LAN NIC leaves it default (absent).
        assert!(net[1].get("mtu").is_none());
        // a MAC-less NIC omits `mac` (cloud-hypervisor auto-generates one).
        assert!(net[0].get("mac").is_none());
    }

    #[test]
    fn virtio_gpu_adds_the_vhost_user_gpu_device_and_blanks_console() {
        let cfg = build_ch_config(&web1());
        let gpu = cfg["gpu"].as_array().expect("gpu array");
        assert_eq!(gpu.len(), 1);
        assert_eq!(
            gpu[0]["socket"],
            json!(gpu_socket_path("web1").to_string_lossy())
        );
        // GPU present ⇒ the display is the GPU, so the virtio-console is Off.
        assert_eq!(cfg["console"]["mode"], json!("Off"));
    }

    #[test]
    fn no_gpu_leaves_no_gpu_device_and_keeps_a_tty_console() {
        let spec = VmSpec::new("plain", 2, 2048, "/p.img"); // virtio_gpu defaults off
        let cfg = build_ch_config(&spec);
        assert!(cfg.get("gpu").is_none());
        assert_eq!(cfg["console"]["mode"], json!("Tty"));
        // no NICs ⇒ no `net` key at all.
        assert!(cfg.get("net").is_none());
    }

    #[test]
    fn firmware_override_is_honored() {
        let spec = VmSpec::new("fw", 1, 512, "/d.img").with_firmware("/opt/edk2/CLOUDHV.fd");
        let cfg = build_ch_config(&spec);
        assert_eq!(cfg["payload"]["firmware"], json!("/opt/edk2/CLOUDHV.fd"));
    }

    #[test]
    fn shared_folders_map_to_fs_entries_with_tag_and_socket() {
        // THE mesh-share acceptance at the config layer: each SharedFolder → one `fs`
        // device carrying the guest mount tag + its per-folder virtiofsd socket.
        let spec = VmSpec::new("web1", 2, 2048, "/home/op/Local/web1.img")
            .with_shared_folder(SharedFolder::mesh_share("/home/op/Mesh/Share"))
            .with_shared_folder(SharedFolder::new("ref", "/srv/ref").with_read_only(true));
        let cfg = build_ch_config(&spec);
        let fs = cfg["fs"].as_array().expect("fs array");
        assert_eq!(fs.len(), 2);
        // folder 0: the mesh-share, tagged + pointed at its per-folder virtiofsd socket.
        assert_eq!(fs[0]["tag"], json!(MESH_SHARE_TAG));
        assert_eq!(
            fs[0]["socket"],
            json!(virtiofs_socket_path("web1", MESH_SHARE_TAG).to_string_lossy())
        );
        assert_eq!(fs[0]["num_queues"], json!(1));
        assert_eq!(fs[0]["queue_size"], json!(1024));
        // folder 1: a second export gets its own tag + its own socket (no collision).
        assert_eq!(fs[1]["tag"], json!("ref"));
        assert_eq!(
            fs[1]["socket"],
            json!(virtiofs_socket_path("web1", "ref").to_string_lossy())
        );
        // host_path + read_only are the virtiofsd launcher's concern, not CH's — they
        // never appear in the `fs` entry (mirrors the dual-homed NIC role).
        assert!(fs[0].get("host_path").is_none());
        assert!(fs[1].get("read_only").is_none());
    }

    #[test]
    fn no_shared_folders_leaves_no_fs_key() {
        let cfg = build_ch_config(&VmSpec::new("plain", 2, 2048, "/p.img"));
        assert!(cfg.get("fs").is_none());
    }

    #[test]
    fn whole_config_equals_the_expected_value() {
        // A full structural snapshot of the minimal headless spec → the exact
        // VmConfig, so any drift in the mapping is caught.
        let spec = VmSpec::new("snap", 2, 1024, "/home/op/Local/snap.img")
            .with_nic(Nic::mesh("mvm-snap-mesh"));
        let cfg = build_ch_config(&spec);
        assert_eq!(
            cfg,
            json!({
                "cpus": { "boot_vcpus": 2, "max_vcpus": 2 },
                "memory": { "size": 1024u64 * 1024 * 1024 },
                "payload": { "firmware": DEFAULT_FIRMWARE },
                "disks": [ { "path": "/home/op/Local/snap.img", "readonly": false } ],
                "rng": { "src": "/dev/urandom" },
                "serial": { "mode": "Tty" },
                "console": { "mode": "Tty" },
                "net": [ { "tap": "mvm-snap-mesh" } ],
            })
        );
    }
}
