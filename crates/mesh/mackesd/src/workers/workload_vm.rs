//! Pure libvirt-domain construction owned by the Workload reconciler.
//!
//! This deliberately contains no Bus, lifecycle request, polling, or command
//! execution path.  [`super::workload_compute`] is the sole caller and owns the
//! bounded `qemu-img`/`virsh` side effects around these deterministic helpers.

/// Default libvirt network for managed Workload VMs.
pub const DEFAULT_NETWORK: &str = "default";

/// The small, reconciler-owned subset of a VM definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmDomainSpec {
    /// Stable libvirt domain name derived from the Workload id.
    pub name: String,
    /// Admitted vCPU count.
    pub vcpus: u32,
    /// Admitted guest memory in MiB.
    pub ram_mb: u64,
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
/// broker receives it over the peer-to-peer libvirt D-Bus connection.  SPICE is
/// retained as an independent loopback-only recovery path.
#[must_use]
pub fn build_domain_xml(spec: &VmDomainSpec, disk_path: &str) -> String {
    format!(
        "<domain type='kvm'>\n\
         \x20 <name>{name}</name>\n\
         \x20 <memory unit='MiB'>{memory}</memory>\n\
         \x20 <currentMemory unit='MiB'>{memory}</currentMemory>\n\
         \x20 <vcpu placement='static'>{vcpus}</vcpu>\n\
         \x20 <os>\n\
         \x20   <type arch='x86_64' machine='q35'>hvm</type>\n\
         \x20   <boot dev='hd'/>\n\
         \x20 </os>\n\
         \x20 <features><acpi/><apic/></features>\n\
         \x20 <cpu mode='host-passthrough' check='none'/>\n\
         \x20 <clock offset='utc'/>\n\
         \x20 <on_poweroff>destroy</on_poweroff>\n\
         \x20 <on_reboot>restart</on_reboot>\n\
         \x20 <on_crash>destroy</on_crash>\n\
         \x20 <resource><partition>/machine.slice</partition></resource>\n\
         \x20 <devices>\n\
         \x20   <disk type='file' device='disk'>\n\
         \x20     <driver name='qemu' type='qcow2'/>\n\
         \x20     <source file='{disk}'/>\n\
         \x20     <target dev='vda' bus='virtio'/>\n\
         \x20   </disk>\n\
         \x20   <interface type='network'><source network='{network}'/><model type='virtio'/></interface>\n\
         \x20   <console type='pty'/>\n\
         \x20   <channel type='unix'><target type='virtio' name='org.qemu.guest_agent.0'/></channel>\n\
         \x20   <graphics type='dbus' p2p='yes'><listen type='none'/></graphics>\n\
         \x20   <graphics type='spice' autoport='yes'><listen type='address' address='127.0.0.1'/></graphics>\n\
         \x20   <video><model type='virtio'><acceleration accel3d='yes'/></model></video>\n\
         \x20   <memballoon model='virtio'/>\n\
         \x20   <sound model='virtio'/>\n\
         \x20   <audio id='1' type='pipewire'><output name='mde-vms' streamName='vm-{name}' latency='40'/></audio>\n\
         \x20 </devices>\n\
         </domain>\n",
        name = xml_escape(&spec.name),
        memory = spec.ram_mb,
        vcpus = spec.vcpus,
        disk = xml_escape(disk_path),
        network = xml_escape(spec.network_or_default()),
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
                network: None,
            },
            "/var/lib/mde-vms/a'&.qcow2",
        );
        assert!(xml.contains("<graphics type='dbus' p2p='yes'>"));
        assert!(xml.contains("address='127.0.0.1'"));
        assert!(xml.contains("guest&lt;&amp;"));
        assert!(xml.contains("a&apos;&amp;.qcow2"));
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
