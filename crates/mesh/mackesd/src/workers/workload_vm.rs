//! Pure libvirt-domain construction owned by the Workload reconciler.
//!
//! This deliberately contains no Bus, lifecycle request, polling, or command
//! execution path.  [`super::workload_compute`] is the sole caller and owns the
//! bounded `qemu-img`/`virsh` side effects around these deterministic helpers.

/// Default libvirt network for managed Workload VMs.
pub const DEFAULT_NETWORK: &str = "default";

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
    /// guest request, CPU 0 is kept outside every QEMU thread affinity mask so
    /// Dom0 always has one VM-free execution lane.
    pub host_threads: u32,
    /// Libvirt network name. `None` selects [`DEFAULT_NETWORK`].
    pub network: Option<String>,
}

impl VmDomainSpec {
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
pub fn build_domain_xml(spec: &VmDomainSpec, disk_path: &str) -> String {
    let cpu_tune = if spec.host_threads > spec.vcpus && spec.vcpus > 0 {
        let guest_cpus = (1..=spec.vcpus)
            .map(|cpu| cpu.to_string())
            .collect::<Vec<_>>();
        let cpuset = guest_cpus.join(",");
        let pins = guest_cpus
            .iter()
            .enumerate()
            .map(|(vcpu, cpu)| format!("    <vcpupin vcpu='{vcpu}' cpuset='{cpu}'/>\n"))
            .collect::<String>();
        format!(
            "  <cputune>\n{pins}    <emulatorpin cpuset='{cpuset}'/>\n    <iothreadpin iothread='1' cpuset='{cpuset}'/>\n  </cputune>\n"
        )
    } else {
        String::new()
    };
    // A queue per admitted guest vCPU avoids serializing desktop traffic on one
    // virtqueue. Keep the single-queue fallback whenever the host cannot retain
    // its required VM-free Dom0 thread, and cap queue-created host work.
    let network_queues = if spec.host_threads > spec.vcpus && spec.vcpus > 0 {
        spec.vcpus.min(MAX_VIRTIO_NET_QUEUES)
    } else {
        1
    };
    format!(
        "<domain type='kvm'>\n\
         \x20 <name>{name}</name>\n\
         \x20 <memory unit='MiB'>{memory}</memory>\n\
         \x20 <currentMemory unit='MiB'>{memory}</currentMemory>\n\
         \x20 <vcpu placement='static'>{vcpus}</vcpu>\n\
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
         \x20     <driver name='qemu' type='qcow2' cache='none' io='native' iothread='1'/>\n\
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
        cpu_tune = cpu_tune,
        disk = xml_escape(disk_path),
        network = xml_escape(spec.network_or_default()),
        network_queues = network_queues,
    )
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
        );
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
            .contains("<driver name='qemu' type='qcow2' cache='none' io='native' iothread='1'/>"));
        assert!(xml.contains("<vcpupin vcpu='0' cpuset='1'/>"));
        assert!(xml.contains("<vcpupin vcpu='1' cpuset='2'/>"));
        assert!(xml.contains("<emulatorpin cpuset='1,2'/>"));
        assert!(xml.contains("<iothreadpin iothread='1' cpuset='1,2'/>"));
        assert!(xml.contains("<driver queues='2'/>"));
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
        };

        assert!(domain(3, 4).contains("<driver queues='3'/>"));
        assert!(domain(4, 4).contains("<driver queues='1'/>"));
        assert!(domain(64, 65).contains("<driver queues='8'/>"));
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
