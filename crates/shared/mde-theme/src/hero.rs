//! PLANES-2 (H1/H2/H5/H6/H7) — monochrome **hero** line-art for the
//! primary-service Workbench panels.
//!
//! Each [`Hero`] is an *original* monochrome line-art glyph (NOT the
//! upstream project logo — H2/H5) baked in at compile time, drawn with
//! `stroke="currentColor"` so the renderer tints it with the single
//! [`HERO_STROKE`] Carbon token (§4). A panel for a primary service
//! (Ansible / LizardFS / Nebula / Fedora / …) shows its hero in the
//! header band at 96–128 px (H3/H4) with the service NAME + live version
//! caption (H8); the art always renders, even when the service isn't
//! installed (H10 — the panel says so in text, the hero stays).

use crate::carbon;
use crate::color::Rgba;

/// The single-sourced stroke colour for all hero line-art (§4): Carbon
/// Gray 50 — a calm mid-stroke that reads cleanly on the Gray-100 dark
/// surface without competing with content. Change it only here.
pub const HERO_STROKE: Rgba = carbon::GRAY_50;

/// A primary-service hero glyph. Order is stable (drives any indexed
/// rendering); add new services at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hero {
    /// Ansible automation platform — configuration management and playbook runner.
    Ansible,
    /// Nebula overlay network — the encrypted mesh fabric connecting all nodes.
    Nebula,
    /// Fedora Linux — the host operating system and DNF package substrate.
    Fedora,
    /// Netdata real-time monitoring — per-node metrics and alerting.
    Netdata,
    /// Podman container runtime — rootless OCI containers on each node.
    Podman,
    /// libvirt virtualisation manager — KVM guest lifecycle and networking.
    Libvirt,
    /// COSMIC desktop environment — the Rust-native Wayland shell on the workbench host.
    Cosmic,
    /// systemd init and service manager — unit supervision across the fleet.
    Systemd,
    /// Remmina remote desktop client — RDP/VNC/SSH sessions to mesh nodes.
    Remmina,
    /// PipeWire audio/video graph — low-latency A/V routing on the workbench host.
    PipeWire,
    /// rustls TLS library — memory-safe TLS for all mesh service endpoints.
    Rustls,
    /// Generic VPN indicator — shown when no more-specific overlay hero applies.
    Vpn,
    /// Syncthing full-mesh file sync (SUBSTRATE-V2) — the mesh **file plane**.
    /// Every node syncs the `/mnt/mesh-storage` folder peer-to-peer over the
    /// Nebula overlay (a plain directory, no FUSE); it replaced `LizardFS` as
    /// the mesh storage substrate. Coordination lives in etcd, not here.
    Syncthing,
}

impl Hero {
    /// Every hero, in stable order.
    #[must_use]
    pub const fn all() -> [Self; 13] {
        [
            Self::Ansible,
            Self::Nebula,
            Self::Fedora,
            Self::Netdata,
            Self::Podman,
            Self::Libvirt,
            Self::Cosmic,
            Self::Systemd,
            Self::Remmina,
            Self::PipeWire,
            Self::Rustls,
            Self::Vpn,
            Self::Syncthing,
        ]
    }

    /// Operator-facing service name shown in the hero caption (H8).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ansible => "Ansible",
            Self::Nebula => "Nebula",
            Self::Fedora => "Fedora",
            Self::Netdata => "Netdata",
            Self::Podman => "Podman",
            Self::Libvirt => "libvirt",
            Self::Cosmic => "COSMIC",
            Self::Systemd => "systemd",
            Self::Remmina => "Remmina",
            Self::PipeWire => "PipeWire",
            Self::Rustls => "rustls",
            Self::Vpn => "VPN",
            Self::Syncthing => "Syncthing",
        }
    }

    /// The baked monochrome line-art SVG (`stroke="currentColor"`).
    #[must_use]
    pub const fn svg_bytes(self) -> &'static [u8] {
        match self {
            Self::Ansible => include_bytes!("../../../../assets/heroes/ansible.svg"),
            Self::Nebula => include_bytes!("../../../../assets/heroes/nebula.svg"),
            Self::Fedora => include_bytes!("../../../../assets/heroes/fedora.svg"),
            Self::Netdata => include_bytes!("../../../../assets/heroes/netdata.svg"),
            Self::Podman => include_bytes!("../../../../assets/heroes/podman.svg"),
            Self::Libvirt => include_bytes!("../../../../assets/heroes/libvirt.svg"),
            Self::Cosmic => include_bytes!("../../../../assets/heroes/cosmic.svg"),
            Self::Systemd => include_bytes!("../../../../assets/heroes/systemd.svg"),
            Self::Remmina => include_bytes!("../../../../assets/heroes/remmina.svg"),
            Self::PipeWire => include_bytes!("../../../../assets/heroes/pipewire.svg"),
            Self::Rustls => include_bytes!("../../../../assets/heroes/rustls.svg"),
            Self::Vpn => include_bytes!("../../../../assets/heroes/vpn.svg"),
            Self::Syncthing => include_bytes!("../../../../assets/heroes/syncthing.svg"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_stroke_is_carbon_gray_50() {
        // §4 — the hero stroke is single-sourced to a Carbon ramp value,
        // not an ad-hoc literal. Pin it so a drift is a test failure.
        assert_eq!(HERO_STROKE, carbon::GRAY_50);
    }

    #[test]
    fn every_hero_bakes_a_nonempty_currentcolor_svg() {
        for h in Hero::all() {
            let svg = std::str::from_utf8(h.svg_bytes()).expect("utf-8 svg");
            assert!(svg.starts_with("<svg"), "{h:?} not an svg");
            assert!(svg.contains("</svg>"), "{h:?} svg truncated");
            // currentColor so HERO_STROKE tints it (no baked-in colour).
            assert!(
                svg.contains("currentColor"),
                "{h:?} must stroke currentColor"
            );
            // line-art, not a filled logo (H2/H5).
            assert!(
                svg.contains("fill=\"none\""),
                "{h:?} must be line-art (fill none)"
            );
        }
    }

    #[test]
    fn all_fourteen_services_present_with_names() {
        let names: Vec<&str> = Hero::all().iter().map(|h| h.name()).collect();
        assert_eq!(names.len(), 13);
        for expected in [
            "Ansible",
            "Nebula",
            "Fedora",
            "COSMIC",
            "rustls",
            "VPN",
            "Syncthing",
        ] {
            assert!(names.contains(&expected), "missing hero name {expected}");
        }
        // names are distinct.
        let uniq: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(uniq.len(), names.len(), "duplicate hero name");
    }

    #[test]
    fn syncthing_is_the_mesh_file_plane_hero() {
        // SUBSTRATE-V2 — the Syncthing hero is the mesh **file plane**; it
        // replaced the LizardFS hero on the Mesh Storage panel. Pin its name +
        // that its art is a non-empty currentColor line-art glyph (the generic
        // sweep already covers `all()`, but Syncthing is load-bearing for the
        // Mesh Storage surface, so guard it explicitly).
        assert_eq!(Hero::Syncthing.name(), "Syncthing");
        let svg = std::str::from_utf8(Hero::Syncthing.svg_bytes()).expect("utf-8 svg");
        assert!(svg.starts_with("<svg"), "syncthing hero not an svg");
        assert!(svg.contains("</svg>"), "syncthing hero svg truncated");
        assert!(
            svg.contains("currentColor"),
            "syncthing hero must stroke currentColor"
        );
        assert!(
            svg.contains("fill=\"none\""),
            "syncthing hero must be line-art"
        );
    }
}
