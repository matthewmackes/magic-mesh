//! Pure libvirt-domain construction owned by the Workload reconciler.
//!
//! This deliberately contains no Bus, lifecycle request, polling, or command
//! execution path.  [`super::workload_compute`] is the sole caller and owns the
//! bounded `qemu-img`/`virsh` side effects around these deterministic helpers.

/// Default libvirt network for managed Workload VMs.
pub const DEFAULT_NETWORK: &str = "default";
/// Managed VM overlay root. Domain XML must never attach a path outside it.
const MANAGED_VM_ROOT: &str = "/var/lib/mde-vms";

/// Bound virtio-net queue fan-out so a large admitted VM cannot turn one NIC
/// into an unbounded host task/FD multiplier. Eight queues are enough to spread
/// workstation traffic without exceeding the local I/O budget.
const MAX_VIRTIO_NET_QUEUES: u32 = 8;

/// The small, reconciler-owned subset of a VM definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmDomainSpec {
    /// Stable libvirt domain name derived from the Workload id.
    pub name: String,
    /// Admitted vCPU count.
    pub vcpus: u32,
    /// Admitted guest memory in MiB.
    pub ram_mb: u64,
    /// Hardware threads visible to the reconciler. When capacity exceeds the
    /// guest request, CPU 0 is kept outside the shared QEMU affinity pool so
    /// Dom0 always has one VM-free execution lane. Individual vCPUs are not
    /// pinned because every admitted VM shares this pool.
    pub host_threads: u32,
    /// Libvirt network name. `None` selects [`DEFAULT_NETWORK`].
    pub network: Option<String>,
}

/// Why a VM domain definition was refused before it could reach libvirt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmDomainSpecError {
    /// A guest must have at least one vCPU and one MiB of memory.
    EmptyResources,
    /// The host must retain one CPU lane for Dom0 and QEMU services.
    NoDom0Reserve,
    /// The disk attachment is outside the reconciler-owned VM pool.
    UnsafeDiskPath,
}

impl std::fmt::Display for VmDomainSpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyResources => formatter.write_str("VM resources must be non-zero"),
            Self::NoDom0Reserve => formatter.write_str("VM requires one host CPU reserved for Dom0"),
            Self::UnsafeDiskPath => {
                formatter.write_str("VM disk attachment is outside the managed VM pool")
            }
        }
    }
}

impl VmDomainSpec {
    fn validate(&self) -> Result<(), VmDomainSpecError> {
        if self.vcpus == 0 || self.ram_mb == 0 {
            return Err(VmDomainSpecError::EmptyResources);
        }
        if self.host_threads <= self.vcpus {
            return Err(VmDomainSpecError::NoDom0Reserve);
        }
        Ok(())
    }

    /// The configured network, or the managed default.
    #[must_use]
    pub fn network_or_default(&self) -> &str {
        self.network.as_deref().unwrap_or(DEFAULT_NETWORK)
    }
}

/// `qemu-img create` argv for a managed copy-on-write disk overlay.
#[must_use]
pub fn build_qemu_img_argv(image: Option<&str>, dest: &str, disk_gb: u64) -> Vec<String> {
    let mut args = vec!["create".into(), "-f".into(), "qcow2".into()];
    if let Some(base) = image {
        args.extend(["-b".into(), base.into(), "-F".into(), "qcow2".into()]);
    }
    args.push(dest.into());
    if disk_gb > 0 {
        args.push(format!("{disk_gb}G"));
    }
    args
}

#[must_use]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build the local Display1-first libvirt domain definition for one Workload.
///
/// The QEMU graphics FD remains local: the reconciler's authenticated Display1
/// broker receives it over the peer-to-peer libvirt D-Bus connection. Guest RDP
/// is the independent recovery path; QEMU rejects a SPICE head beside D-Bus GL.
/// Audio is local-only too: QEMU connects to the seat user's hardened
/// PipeWire-Pulse loopback endpoint instead of guessing at a system QEMU
/// user's nonexistent PipeWire runtime directory.
#[must_use]
pub fn build_domain_xml(
    spec: &VmDomainSpec,
    disk_path: &str,
) -> Result<String, VmDomainSpecError> {
    spec.validate()?;
    let disk = std::path::Path::new(disk_path);
    let managed_root = std::path::Path::new(MANAGED_VM_ROOT);
    if !disk.is_absolute()
        || !disk.starts_with(managed_root)
        || disk.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
        || disk.parent() != Some(managed_root)
        || disk.extension().and_then(|extension| extension.to_str()) != Some("qcow2")
    {
        return Err(VmDomainSpecError::UnsafeDiskPath);
    }
    let guest_cpuset = format!("1-{}", spec.host_threads - 1);
    let cpu_tune = {
        format!(
            "  <cputune>\n    <emulatorpin cpuset='{guest_cpuset}'/>\n    <iothreadpin iothread='1' cpuset='{guest_cpuset}'/>\n  </cputune>\n"
        )
    };
    // A queue per admitted guest vCPU avoids serializing desktop traffic on one
    // virtqueue while the cap bounds queue-created host work.
    let network_queues = spec.vcpus.min(MAX_VIRTIO_NET_QUEUES);
    Ok(format!(
        "<domain type='kvm'>\n\
         \x20 <name>{name}</name>\n\
         \x20 <memory unit='MiB'>{memory}</memory>\n\
         \x20 <currentMemory unit='MiB'>{memory}</currentMemory>\n\
         \x20 <vcpu placement='static'{vcpu_cpuset}>{vcpus}</vcpu>\n\
         \x20 <iothreads>1</iothreads>\n\
         {cpu_tune}\
         \x20 <os>\n\
         \x20   <type arch='x86_64' machine='q35'>hvm</type>\n\
         \x20   <boot dev='hd'/>\n\
         \x20 </os>\n\
         \x20 <features><acpi/><apic/></features>\n\
         \x20 <cpu mode='host-passthrough' check='none'><topology sockets='1' dies='1' cores='{vcpus}' threads='1'/></cpu>\n\
         \x20 <clock offset='utc'/>\n\
         \x20 <on_poweroff>destroy</on_poweroff>\n\
         \x20 <on_reboot>restart</on_reboot>\n\
         \x20 <on_crash>destroy</on_crash>\n\
         \x20 <resource><partition>/machine.slice</partition></resource>\n\
         \x20 <devices>\n\
         \x20   <disk type='file' device='disk'>\n\
         \x20     <driver name='qemu' type='qcow2' cache='none' io='native' iothread='1' discard='unmap'/>\n\
         \x20     <source file='{disk}'/>\n\
         \x20     <target dev='vda' bus='virtio'/>\n\
         \x20   </disk>\n\
         \x20   <interface type='network'><source network='{network}'/><model type='virtio'/><driver queues='{network_queues}'/></interface>\n\
         \x20   <console type='pty'/>\n\
         \x20   <channel type='unix'><target type='virtio' name='org.qemu.guest_agent.0'/></channel>\n\
         \x20   <graphics type='dbus' p2p='yes'><listen type='none'/><gl enable='yes'/></graphics>\n\
         \x20   <video><model type='virtio'><acceleration accel3d='yes'/></model></video>\n\
         \x20   <memballoon model='virtio'/>\n\
         \x20   <sound model='virtio'/>\n\
         \x20   <audio id='1' type='pulseaudio' serverName='tcp:127.0.0.1:4713'>\n\
         \x20     <input streamName='vm-{name}-capture'/>\n\
         \x20     <output streamName='vm-{name}-playback'/>\n\
         \x20   </audio>\n\
         \x20 </devices>\n\
         </domain>\n",
        name = xml_escape(&spec.name),
        memory = spec.ram_mb,
        vcpus = spec.vcpus,
        vcpu_cpuset = format!(" cpuset='{guest_cpuset}'"),
        cpu_tune = cpu_tune,
        disk = xml_escape(disk_path),
        network = xml_escape(spec.network_or_default()),
        network_queues = network_queues,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_uses_display1_and_escapes_untrusted_fields() {
        let xml = build_domain_xml(
            &VmDomainSpec {
                name: "guest<&".to_string(),
                vcpus: 2,
                ram_mb: 4096,
                host_threads: 4,
                network: None,
            },
            "/var/lib/mde-vms/a'&.qcow2",
        )
        .expect("valid VM domain spec");
        assert!(xml.contains("<graphics type='dbus' p2p='yes'>"));
        assert!(xml.contains("<gl enable='yes'/></graphics>"));
        assert_eq!(xml.matches("<graphics type=").count(), 1);
        assert!(!xml.contains("type='spice'"));
        assert!(xml.contains("guest&lt;&amp;"));
        assert!(xml.contains("a&apos;&amp;.qcow2"));
        assert!(xml.contains("<audio id='1' type='pulseaudio' serverName='tcp:127.0.0.1:4713'>"));
        assert!(xml.contains("<input streamName='vm-guest&lt;&amp;-capture'/>"));
        assert!(xml.contains("<output streamName='vm-guest&lt;&amp;-playback'/>"));
        assert!(!xml.contains("type='pipewire'"));
        assert!(xml.contains("<iothreads>1</iothreads>"));
        assert!(xml.contains("<topology sockets='1' dies='1' cores='2' threads='1'/>"));
        assert!(xml
            .contains("<driver name='qemu' type='qcow2' cache='none' io='native' iothread='1' discard='unmap'/>"));
        assert!(xml.contains("<vcpu placement='static' cpuset='1-3'>2</vcpu>"));
        assert!(xml.contains("<emulatorpin cpuset='1-3'/>"));
        assert!(xml.contains("<iothreadpin iothread='1' cpuset='1-3'/>"));
        assert!(!xml.contains("<vcpupin "));
        assert!(xml.contains("<driver queues='2'/>"));
    }

    #[test]
    fn shared_guest_pool_avoids_colliding_per_vcpu_pins() {
        let spec = VmDomainSpec {
            name: "shared-pool".into(),
            vcpus: 2,
            ram_mb: 1024,
            host_threads: 8,
            network: None,
        };

        let xml = build_domain_xml(&spec, "/var/lib/mde-vms/shared-pool.qcow2")
            .expect("valid VM domain spec");

        assert!(xml.contains("<vcpu placement='static' cpuset='1-7'>2</vcpu>"));
        assert!(xml.contains("<emulatorpin cpuset='1-7'/>"));
        assert!(xml.contains("<iothreadpin iothread='1' cpuset='1-7'/>"));
        assert!(!xml.contains("<vcpupin "));
        assert!(!xml.contains("cpuset='1,2'"));
    }

    #[test]
    fn network_queue_fanout_preserves_dom0_capacity_and_is_bounded() {
        let domain = |vcpus, host_threads| {
            build_domain_xml(
                &VmDomainSpec {
                    name: "queue-test".into(),
                    vcpus,
                    ram_mb: 1024,
                    host_threads,
                    network: None,
                },
                "/var/lib/mde-vms/queue-test.qcow2",
            )
            .expect("valid VM domain spec")
        };

        assert!(domain(3, 4).contains("<driver queues='3'/>"));
        assert!(domain(1, 2).contains("<driver queues='1'/>"));
        assert!(domain(64, 65).contains("<driver queues='8'/>"));
    }

    #[test]
    fn definition_refuses_to_overcommit_dom0_cpu_reserve() {
        let spec = VmDomainSpec {
            name: "overcommitted".into(),
            vcpus: 4,
            ram_mb: 4096,
            host_threads: 4,
            network: None,
        };

        assert_eq!(
            build_domain_xml(&spec, "/var/lib/mde-vms/overcommitted.qcow2"),
            Err(VmDomainSpecError::NoDom0Reserve)
        );
    }

    #[test]
    fn definition_refuses_disk_attachment_outside_managed_pool() {
        let spec = VmDomainSpec {
            name: "unsafe-disk".into(),
            vcpus: 1,
            ram_mb: 1024,
            host_threads: 2,
            network: None,
        };

        for path in [
            "/var/lib/mde-vms/../etc/shadow.qcow2",
            "/var/lib/mde-vms/nested/guest.qcow2",
            "/tmp/guest.qcow2",
            "var/lib/mde-vms/guest.qcow2",
            "/var/lib/mde-vms/guest.raw",
        ] {
            assert_eq!(
                build_domain_xml(&spec, path),
                Err(VmDomainSpecError::UnsafeDiskPath),
                "unsafe disk path must be rejected: {path}"
            );
        }
    }

    #[test]
    fn overlay_argv_never_mutates_its_base() {
        assert_eq!(
            build_qemu_img_argv(Some("/images/base.qcow2"), "/pool/guest.qcow2", 40),
            vec![
                "create".to_string(),
                "-f".to_string(),
                "qcow2".to_string(),
                "-b".to_string(),
                "/images/base.qcow2".to_string(),
                "-F".to_string(),
                "qcow2".to_string(),
                "/pool/guest.qcow2".to_string(),
                "40G".to_string(),
            ]
        );
    }
}
