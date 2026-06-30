//! The **xcp-ng service set** the [`Role::Xcpng`](crate::Role::Xcpng) deployment
//! mirrors.
//!
//! An MCNF XCP-NG host is a Xen virtualization host that runs (or supervises) the
//! same daemon set as the upstream **xcp-ng project** — the XAPI toolstack that
//! turns a bare Xen host into a managed, pool-capable hypervisor serving VM
//! desktops to the mesh. This module is the single source of that catalog: every
//! service, its systemd unit, and what it does, so the role's provisioning, the
//! `mackesd` health checks, and the Workbench host view all agree on what an
//! XCP-NG node provides.
//!
//! The catalog mirrors the toolstack's own boot order (Xen → xenstore → the
//! message switch → the per-subsystem daemons → xapi last, the management brain).

/// One service in the xcp-ng toolstack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XcpngService {
    /// Short canonical id — the key under which the service is reported in role /
    /// host status.
    pub id: &'static str,
    /// The systemd unit that supervises it on the host.
    pub unit: &'static str,
    /// One-line role in the toolstack.
    pub summary: &'static str,
}

impl XcpngService {
    const fn new(id: &'static str, unit: &'static str, summary: &'static str) -> Self {
        Self { id, unit, summary }
    }
}

/// The full xcp-ng service set an [`Role::Xcpng`](crate::Role::Xcpng) host
/// provisions, mirroring the xcp-ng project — the Xen hypervisor plus the XAPI
/// toolstack daemons. Ordered boot/dependency-first (xapi, the management brain,
/// comes up last once its subsystems are ready).
pub static XCPNG_SERVICES: &[XcpngService] = &[
    XcpngService::new(
        "xen",
        "xen-watchdog.service",
        "The Xen type-1 hypervisor — the bare-metal substrate every guest runs on.",
    ),
    XcpngService::new(
        "xenstored",
        "xenstored.service",
        "XenStore — the shared inter-domain configuration/status database.",
    ),
    XcpngService::new(
        "xenconsoled",
        "xenconsoled.service",
        "Serves the text consoles of the guest domains.",
    ),
    XcpngService::new(
        "message-switch",
        "message-switch.service",
        "The XCP message switch — the IPC bus between the toolstack daemons.",
    ),
    XcpngService::new(
        "forkexecd",
        "forkexecd.service",
        "Safe, audited subprocess execution for the toolstack.",
    ),
    XcpngService::new(
        "xcp-rrdd",
        "xcp-rrdd.service",
        "Round-robin metrics daemon — per-host and per-VM performance RRDs.",
    ),
    XcpngService::new(
        "xcp-networkd",
        "xcp-networkd.service",
        "Host network configuration — bridges, bonds, and VLANs.",
    ),
    XcpngService::new(
        "squeezed",
        "squeezed.service",
        "Memory-ballooning daemon — dynamic guest memory (DMC).",
    ),
    XcpngService::new(
        "xenopsd",
        "xenopsd-xc.service",
        "The Xen operations daemon — VM lifecycle: start / stop / suspend / migrate.",
    ),
    XcpngService::new(
        "sm",
        "sm.service",
        "Storage Manager — storage repositories (SRs) and virtual disks (VDIs).",
    ),
    XcpngService::new(
        "mpathalert",
        "mpathalert.service",
        "Multipath storage-path monitoring and alerting.",
    ),
    XcpngService::new(
        "perfmon",
        "perfmon.service",
        "Performance monitoring and alarm evaluation.",
    ),
    XcpngService::new(
        "v6d",
        "v6d.service",
        "Feature daemon — gates the xcp-ng edition feature flags.",
    ),
    XcpngService::new(
        "xha",
        "xha.service",
        "High-availability daemon — pool HA fencing and VM failover.",
    ),
    XcpngService::new(
        "stunnel",
        "stunnel@xapi.service",
        "TLS termination for the XAPI management interface (:443).",
    ),
    XcpngService::new(
        "xapi",
        "xapi.service",
        "The XenAPI toolstack — the management brain: pool, VMs, SRs, the XE/XO API.",
    ),
];

#[cfg(test)]
mod tests {
    use super::XCPNG_SERVICES;

    #[test]
    fn catalog_mirrors_the_full_xcpng_toolstack() {
        // The role must mirror the xcp-ng project — every load-bearing daemon is
        // present and uniquely identified.
        assert!(
            XCPNG_SERVICES.len() >= 14,
            "the xcp-ng catalog looks short ({} services)",
            XCPNG_SERVICES.len()
        );
        for must in [
            "xen",
            "xapi",
            "xenopsd",
            "xenstored",
            "xcp-rrdd",
            "xcp-networkd",
            "squeezed",
            "sm",
            "xha",
        ] {
            assert!(
                XCPNG_SERVICES.iter().any(|s| s.id == must),
                "the xcp-ng catalog is missing `{must}`"
            );
        }
        // ids and units are unique, and every entry is fully populated.
        for (i, a) in XCPNG_SERVICES.iter().enumerate() {
            assert!(!a.id.is_empty() && !a.unit.is_empty() && !a.summary.is_empty());
            for b in &XCPNG_SERVICES[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate service id `{}`", a.id);
                assert_ne!(a.unit, b.unit, "duplicate unit `{}`", a.unit);
            }
        }
    }
}
